extern crate libc;

use syntax::Name;
use syntax::token::name;
use super::{JsEnv, JsFunction, JsValue, JsItem, JsDescriptor, JsScope, JsType, GC_OBJECT};
use gc::{Local, Ptr};
use ::{JsResult, JsError};
use self::hash_store::HashStore;
use self::array_store::ArrayStore;
use std::mem;
use self::libc::c_void;
use std::str::FromStr;
use std::u32;

mod hash_store;
mod array_store;

pub struct JsObject {
	class: Option<Name>,
	value: Option<JsValue>,
	function: Option<JsFunction>,
	prototype: Ptr<JsObject>,
	scope: Ptr<JsScope>,
	store: StorePtr,
	extensible: bool
}

impl JsObject {
	pub fn new(env: &JsEnv, ty: JsStoreType) -> JsObject {
		let store = unsafe {
			match ty {
				JsStoreType::Hash => StorePtr::new(HashStore::new_local(env).as_ptr(), ty),
				JsStoreType::Array => StorePtr::new(ArrayStore::new_local(env).as_ptr(), ty)
			}
		};
		
		JsObject {
			class: None,
			value: None,
			function: None,
			prototype: Ptr::null(),
			scope: Ptr::null(),
			store: store,
			extensible: true
		}
	}
	
	pub fn new_local(env: &JsEnv, ty: JsStoreType) -> Local<JsObject> {
		let mut result = env.heap.alloc_local(GC_OBJECT);
		*result = Self::new(env, ty);
		result
	}
	
	pub fn new_function(env: &mut JsEnv, function: JsFunction, prototype: Local<JsObject>) -> Local<JsObject> {
		let mut result = Self::new_local(env, JsStoreType::Hash);
		
		let args = match function {
			JsFunction::Native(_, args, _, _) => args,
			JsFunction::Ir(function_ref) => {
				let function = env.ir.get_function(function_ref);
				function.args
			}
		};
		
		result.prototype = prototype.as_ptr();
		result.class = Some(name::FUNCTION_CLASS);
		result.function = Some(function);
		
		let value = JsValue::new_number(args as f64).as_local(env);
		// TODO: This does not seem to be conform the specs. Value should not be configurable.
		result.define_own_property(env, name::LENGTH, JsDescriptor::new_value(value, false, false, true), false).ok();
		
		result
	}
}

impl Local<JsObject> {
	pub fn value(&self, env: &JsEnv) -> Option<Local<JsValue>> {
		if let Some(value) = self.value {
			Some(value.as_local(env))
		} else {
			None
		}
	}
	
	pub fn set_value(&mut self, value: Option<Local<JsValue>>) {
		self.value = value.map(|value| *value);
	}
	
	pub fn function(&self) -> Option<JsFunction> {
		self.function.clone()
	}
	
	pub fn get_key(&self, env: &JsEnv, offset: usize) -> JsStoreKey {
		self.store.get_key(env, offset)
	}

	// 8.12.9 [[DefineOwnProperty]] (P, Desc, Throw)
	fn define_own_object_property(&mut self, env: &mut JsEnv, property: Name, descriptor: JsDescriptor, throw: bool) -> JsResult<bool> {
		let current = self.get_own_property(env, property);
		let extensible = self.is_extensible(env);
		
		match current {
			None => {
				return if !extensible {
					if throw { Err(JsError::new_type(env, ::errors::TYPE_NOT_EXTENSIBLE)) } else { Ok(false) }
				} else {
					if descriptor.is_generic() || descriptor.is_data() {
						JsDescriptor {
							value: Some(descriptor.value(env)),
							get: None,
							set: None,
							writable: Some(descriptor.is_writable()),
							enumerable: Some(descriptor.is_enumerable()),
							configurable: Some(descriptor.is_configurable())
						}
					} else {
						JsDescriptor {
							value: None,
							get: Some(descriptor.get(env)),
							set: Some(descriptor.set(env)),
							writable: Some(descriptor.is_writable()),
							enumerable: Some(descriptor.is_enumerable()),
							configurable: Some(descriptor.is_configurable())
						}
					};
					
					self.store.add(env, property, &descriptor);
					
					Ok(true)
				}
			}
			Some(current) => {
				if descriptor.is_empty() {
					return Ok(true);
				}
				if current.is_same(env, &descriptor) {
					return Ok(true);
				}
				
				fn can_write(env: &JsEnv, current: &JsDescriptor, desc: &JsDescriptor) -> bool {
					if 
						!current.is_configurable() &&
						(desc.configurable == Some(true) || (desc.enumerable == Some(!current.is_enumerable())))
					{
						return false;
					}
					
					if current.is_generic() {
						return true;
					}

					if current.is_data() != desc.is_data() {
						if !current.is_configurable() {
							return false;
						}
						// TODO: Preservation of configurable, enumerable.
						return true;
					}
					
					if current.is_data() && desc.is_data() {
						if !current.is_configurable() {
							if desc.writable == Some(!current.is_writable()) {
								return false;
							}
							if !current.is_writable() {
								if let Some(value) = desc.value {
									if !env.same_value(current.value(env), value) {
										return false;
									}
								}
							}
						}
						
						return true;
					}
					
					if current.is_accessor() && desc.is_accessor() {
						if !current.is_configurable() {
							if let Some(set) = desc.set {
								if !env.same_value(current.set(env), set) {
									return false;
								}
							}
							if let Some(get) = desc.get {
								if !env.same_value(current.get(env), get) {
									return false;
								}
							}
						}
					}
					
					true
				}
				
				if !can_write(env, &current, &descriptor) {
					if throw { Err(JsError::new_type(env, ::errors::TYPE_CANNOT_WRITE)) } else { Ok(false) }
				} else {
					let descriptor = JsDescriptor {
						writable: Some(descriptor.writable.unwrap_or(current.is_writable())),
						enumerable: Some(descriptor.enumerable.unwrap_or(current.is_enumerable())),
						configurable: Some(descriptor.configurable.unwrap_or(current.is_configurable())),
						..descriptor
					};

					self.store.replace(env, property, &descriptor);
					
					Ok(true)
				}
			}
		}
	}
	
	// 15.4.5.1 [[DefineOwnProperty]] ( P, Desc, Throw )
	fn define_own_array_property(&mut self, env: &mut JsEnv, property: Name, descriptor: JsDescriptor, throw: bool) -> JsResult<bool> {
		let mut old_len_desc = self.get_own_property(env, name::LENGTH).unwrap();
		// This is safe because we control the value of length.
		let old_len = old_len_desc.value.unwrap().get_number() as usize;
		
		if property == name::LENGTH {
			return match descriptor.value {
				None => {
					self.define_own_object_property(
						env,
						name::LENGTH,
						descriptor,
						throw
					)
				}
				Some(desc_value) => {
					let mut new_len_desc = descriptor.clone();
					let new_len = try!(desc_value.to_uint32(env)) as usize;
					
					if new_len as f64 != try!(desc_value.to_number(env)) {
						Err(JsError::new_range(env))
					} else {
						new_len_desc.value = Some(JsValue::new_number(new_len as f64).as_local(env));
						
						if new_len >= old_len {
							self.define_own_object_property(
								env,
								name::LENGTH,
								new_len_desc,
								throw
							)
						} else if !old_len_desc.is_writable() {
							if throw { Err(JsError::new_type(env, ::errors::TYPE_CANNOT_WRITE)) } else { Ok(false) }
						} else {
							let new_writable = if new_len_desc.is_writable() {
								true
							} else {
								new_len_desc.writable = Some(true);
								false
							};
							
							let succeeded = try!(self.define_own_object_property(
								env,
								name::LENGTH,
								new_len_desc,
								throw
							));
							if !succeeded {
								return if throw { Err(JsError::new_type(env, ::errors::TYPE_CANNOT_WRITE)) } else { Ok(false) };
							}
							
							/*
							// The array store itself takes care of truncation.
							while new_len < old_len {
								old_len -= 1;
								
								let delete_succeeded = try!(self.delete(
									env,
									Name::from_index(old_len),
									false
								));
								
								if !delete_succeeded {
									new_len_desc.value = Some(JsValue::new_number((old_len + 1) as f64).as_local(env));
									if !new_writable {
										new_len_desc.writable = Some(false);
									}
									try!(self.define_own_object_property(
										env,
										name::LENGTH,
										new_len_desc,
										false
									));
									
									return if throw { Err(JsError::new_type(env, ::errors::TYPE_CANNOT_WRITE)) } else { Ok(false) };
								}
							}
							*/
							
							if !new_writable {
								try!(self.define_own_object_property(
									env,
									name::LENGTH,
									JsDescriptor {
										writable: Some(false),
										..JsDescriptor::default()
									},
									false
								));
							}
							
							Ok(true)
						}
					}
				}
			}
		} else {
			match Self::parse_array_index(env, property) {
				Some(index) => {
					if index >= old_len && !old_len_desc.is_writable() {
						if throw { Err(JsError::new_type(env, ::errors::TYPE_CANNOT_WRITE)) } else { Ok(false) }
					} else {
						let succeeded = try!(self.define_own_object_property(
							env,
							property,
							descriptor,
							false
						));
						
						if !succeeded {
							if throw { Err(JsError::new_type(env, ::errors::TYPE_CANNOT_WRITE)) } else { Ok(false) }
						} else if index >= old_len {
							old_len_desc.value = Some(JsValue::new_number((index + 1) as f64).as_local(env));
							try!(self.define_own_object_property(
								env,
								name::LENGTH,
								old_len_desc,
								false
							));
							
							Ok(true)
						} else {
							self.define_own_object_property(
								env,
								property,
								descriptor,
								throw
							)
						}
					}
				}
				None => {
					self.define_own_object_property(
						env,
						property,
						descriptor,
						throw
					)
				}
			}
		}
	}
	
	// 15.4 Array Objects
	fn is_array_index(env: &JsEnv, name: Name) -> bool {
		Self::parse_array_index(env, name).is_some()
	}
	
	// 15.4 Array Objects
	fn parse_array_index(env: &JsEnv, name: Name) -> Option<usize> {
		match name.index() {
			Some(index) => Some(index),
			None => {
				match u32::from_str(&env.ir.interner().get(name)) {
					Ok(value) => if value != u32::MAX { Some(value as usize) } else { None },
					Err(..) => None
				}
			}
		}
	}
}

impl JsItem for Local<JsObject> {
	fn as_value(&self, env: &JsEnv) -> Local<JsValue> {
		JsValue::new_object(self.as_ptr()).as_local(env)
	}

	// 8.12.1 [[GetOwnProperty]] (P)
	fn get_own_property(&self, env: &JsEnv, property: Name) -> Option<JsDescriptor> {
		self.store.get_value(env, property)
	}
	
	// 8.12.7 [[Delete]] (P, Throw)
	fn delete(&mut self, env: &mut JsEnv, property: Name, throw: bool) -> JsResult<bool> {
		if let Some(desc) = self.get_own_property(env, property) {
			if desc.is_configurable() {
				self.store.remove(env, property);
				Ok(true)
			} else if throw {
				Err(JsError::new_type(env, ::errors::TYPE_CANNOT_DELETE))
			} else {
				Ok(false)
			}
		} else {
			Ok(true)
		}
	}
	
	// 8.12.9 [[DefineOwnProperty]] (P, Desc, Throw)
	// 15.4.5.1 [[DefineOwnProperty]] ( P, Desc, Throw )
	fn define_own_property(&mut self, env: &mut JsEnv, property: Name, descriptor: JsDescriptor, throw: bool) -> JsResult<bool> {
		if self.class == Some(name::ARRAY_CLASS) {
			self.define_own_array_property(env, property, descriptor, throw)
		} else {
			self.define_own_object_property(env, property, descriptor, throw)
		}
	}
	
	fn is_callable(&self, _: &JsEnv) -> bool {
		self.function.is_some()
	}
	
	fn can_construct(&self, _: &JsEnv) -> bool {
		self.function.is_some()
	}
	
	fn has_prototype(&self, _: &JsEnv) -> bool {
		!self.prototype.is_null()
	}
	
	fn prototype(&self, env: &JsEnv) -> Option<Local<JsValue>> {
		if self.prototype.is_null() {
			None
		} else {
			Some(JsValue::new_object(self.prototype).as_local(env))
		}
	}
	
	fn set_prototype(&mut self, _: &JsEnv, prototype: Option<Local<JsValue>>) {
		if let Some(prototype) = prototype {
			if prototype.ty() == JsType::Object {
				self.prototype = prototype.get_object()
			}
		} else {
			self.prototype = Ptr::null()
		}
	}
	
	fn has_class(&self, _: &JsEnv) -> bool {
		self.class.is_some()
	}
	
	fn class(&self, _: &JsEnv) -> Option<Name> {
		self.class
	}
	
	fn set_class(&mut self, _: &JsEnv, class: Option<Name>) {
		self.class = class
	}
	
	fn is_extensible(&self, _: &JsEnv) -> bool {
		self.extensible
	}
	
	// 15.3.5.3 [[HasInstance]] (V)
	fn has_instance(&self, env: &mut JsEnv, object: Local<JsValue>) -> JsResult<bool> {
		if self.function.is_none() {
			Err(JsError::new_type(env, ::errors::TYPE_CANNOT_HAS_INSTANCE))
		} else if object.ty() != JsType::Object {
			Ok(false)
		} else {
			let prototype = try!(self.get(env, name::PROTOTYPE));
			if prototype.ty() != JsType::Object {
				Err(JsError::new_type(env, ::errors::TYPE_CANNOT_HAS_INSTANCE))
			} else {
				let mut object = object;
				
				loop {
					if let Some(object_) = object.prototype(env) {
						object = object_;
						if prototype.get_object().as_ptr() == object.get_object().as_ptr() {
							return Ok(true)
						}
					} else {
						return Ok(false)
					}
				}
			}
		}
	}
	
	fn scope(&self, env: &JsEnv) -> Option<Local<JsScope>> {
		if self.scope.is_null() {
			None
		} else {
			Some(JsValue::new_scope(self.scope).get_scope().as_local(env))
		}
	}
	
	fn set_scope(&mut self, _: &JsEnv, scope: Option<Local<JsScope>>) {
		if let Some(scope) = scope {
			self.scope = scope.as_ptr();
		} else {
			self.scope = Ptr::null();
		}
	}
	
	fn formal_parameters(&self, _: &JsEnv) -> Option<Vec<Name>> {
		unimplemented!();
	}
	
	fn code(&self, _: &JsEnv) -> Option<String> {
		unimplemented!();
	}
	
	fn target_function(&self, _: &JsEnv) -> Option<Local<JsValue>> {
		unimplemented!();
	}
	
	fn bound_this(&self, _: &JsEnv) -> Option<Local<JsValue>> {
		unimplemented!();
	}
	
	fn bound_arguments(&self, _: &JsEnv) -> Option<Local<JsValue>> {
		unimplemented!();
	}
}

trait Store {
	fn add(&mut self, env: &JsEnv, name: Name, value: &JsDescriptor);
	
	fn remove(&mut self, env: &JsEnv, name: Name) -> bool;
	
	fn get_value(&self, env: &JsEnv, name: Name) -> Option<JsDescriptor>;
	
	fn replace(&mut self, env: &JsEnv, name: Name, value: &JsDescriptor) -> bool;
	
	fn get_key(&self, env: &JsEnv, offset: usize) -> JsStoreKey;
}

pub enum JsStoreKey {
	Key(Name, bool),
	Missing,
	End
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum JsStoreType {
	Hash = 1,
	Array = 2
}

struct StorePtr {
	ty: JsStoreType,
	ptr: usize
}

impl StorePtr {
	unsafe fn new<T>(ptr: Ptr<T>, ty: JsStoreType) -> StorePtr {
		StorePtr {
			ty: ty,
			ptr: ptr.as_ptr() as usize
		}
	}
	
	fn get_ptr<T>(&self) -> Ptr<T> {
		Ptr::from_ptr(self.ptr as *const c_void)
	}
	
	fn as_hash(&self, env: &JsEnv) -> Local<HashStore> {
		assert_eq!(self.ty, JsStoreType::Hash);
		
		Local::from_ptr(self.get_ptr(), &env.heap)
	}
	
	fn as_array(&self, env: &JsEnv) -> Local<ArrayStore> {
		assert_eq!(self.ty, JsStoreType::Array);
		
		Local::from_ptr(self.get_ptr(), &env.heap)
	}
}

macro_rules! delegate {
	( $target:expr, $env:expr, $method:ident ( $( $arg:expr ),* ) ) => {
		match $target.ty {
			JsStoreType::Hash => $target.as_hash($env).$method( $( $arg ),* ),
			JsStoreType::Array => $target.as_array($env).$method( $( $arg ),* ),
		}
	}
}

impl Store for StorePtr {
	fn add(&mut self, env: &JsEnv, name: Name, value: &JsDescriptor) {
		delegate!(self, env, add(env, name, value))
	}
	
	fn remove(&mut self, env: &JsEnv, name: Name) -> bool {
		delegate!(self, env, remove(env, name))
	}
	
	fn get_value(&self, env: &JsEnv, name: Name) -> Option<JsDescriptor> {
		delegate!(self, env, get_value(env, name))
	}
	
	fn replace(&mut self, env: &JsEnv, name: Name, value: &JsDescriptor) -> bool {
		delegate!(self, env, replace(env, name, value))
	}
	
	fn get_key(&self, env: &JsEnv, offset: usize) -> JsStoreKey {
		delegate!(self, env, get_key(env, offset))
	}
}

const VALID        : u32 = 0b00001;
const WRITABLE     : u32 = 0b00010;
const ENUMERABLE   : u32 = 0b00100;
const CONFIGURABLE : u32 = 0b01000;
const ACCESSOR     : u32 = 0b10000;

#[derive(Copy, Clone)]
pub struct Entry {
	name: Name,
	flags: u32,
	next: i32,
	value1: JsValue,
	value2: JsValue
}

impl Entry {
	fn empty() -> Entry {
		unsafe { mem::zeroed() }
	}
	
	fn is_valid(&self) -> bool {
		(self.flags & VALID) != 0
	}
	
	fn is_writable(&self) -> bool {
		(self.flags & WRITABLE) != 0
	}
	
	fn is_enumerable(&self) -> bool {
		(self.flags & ENUMERABLE) != 0
	}
	
	fn is_configurable(&self) -> bool {
		(self.flags & CONFIGURABLE) != 0
	}
	
	fn is_accessor(&self) -> bool {
		(self.flags & ACCESSOR) != 0
	}
	
	fn as_property(&self, env: &JsEnv) -> JsDescriptor {
		if self.is_accessor() {
			JsDescriptor {
				value: None,
				get: Some(self.value1.as_local(env)),
				set: Some(self.value2.as_local(env)),
				writable: None,
				enumerable: Some(self.is_enumerable()),
				configurable: Some(self.is_configurable())
			}
		} else {
			JsDescriptor {
				value: Some(self.value1.as_local(env)),
				get: None,
				set: None,
				writable: Some(self.is_writable()),
				enumerable: Some(self.is_enumerable()),
				configurable: Some(self.is_configurable())
			}
		}
	}
	
	
	fn from_descriptor(descriptor: &JsDescriptor, name: Name, next: i32) -> Entry {
		let flags = VALID |
			if descriptor.writable.unwrap_or(true) { WRITABLE } else { 0 } |
			if descriptor.configurable.unwrap_or(true) { CONFIGURABLE } else { 0 } |
			if descriptor.enumerable.unwrap_or(true) { ENUMERABLE } else { 0 } |
			if descriptor.is_accessor() { ACCESSOR } else { 0 };
		
		let value1;
		let value2;
		
		if descriptor.is_accessor() {
			value1 = if let Some(get) = descriptor.get {
				*get
			} else {
				JsValue::new_undefined()
			};
			value2 = if let Some(set) = descriptor.set {
				*set
			} else {
				JsValue::new_undefined()
			};
		} else {
			value1 = if let Some(value) = descriptor.value {
				*value
			} else {
				JsValue::new_undefined()
			};
			value2 = JsValue::new_undefined();
		}
		
		Entry {
			name: name,
			flags: flags,
			next: next,
			value1: value1,
			value2: value2
		}
	}
}
