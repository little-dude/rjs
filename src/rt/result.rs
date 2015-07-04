use gc::Root;
use rt::{JsEnv, JsValue, JsString, JsItem, JsHandle};
use std::fmt;
use std::io;

pub enum JsError {
    Io(io::Error),
    Lex(String),
    Parse(String),
    Reference(String),
    Runtime(Root<JsValue>)
}

impl JsError {
    fn new_error(env: &mut JsEnv, handle: JsHandle, message: Option<&str>, file_name: Option<&str>, line_number: Option<usize>) -> JsResult<Root<JsValue>> {
        // If construction of the error fails, we simply propagate the error itself.
        
        let _scope = env.new_local_scope();
        
        let class = env.handle(handle);
        
        let mut args = Vec::new();
        
        args.push(match message {
            Some(message) => JsString::from_str(env, message).as_value(env),
            None => env.new_undefined()
        });
        args.push(match file_name {
            Some(file_name) => JsString::from_str(env, file_name).as_value(env),
            None => env.new_undefined()
        });
        args.push(match line_number {
            Some(line_number) => env.new_number(line_number as f64),
            None => env.new_undefined()
        });
        
        let obj = try!(class.construct(env, args));
        
        Ok(obj.as_root(env))
    }
    
    pub fn new_runtime(env: &mut JsEnv, handle: JsHandle, message: Option<&str>, file_name: Option<&str>, line_number: Option<usize>) -> JsError {
        match Self::new_error(env, handle, message, file_name, line_number) {
            Ok(error) => JsError::Runtime(error),
            Err(error) => error
        }
    }
    
    pub fn new_type(env: &mut JsEnv, message: &str) -> JsError {
        Self::new_runtime(env, JsHandle::TypeError, Some(message), None, None)
    }
    
    pub fn new_range(env: &mut JsEnv) -> JsError {
        Self::new_runtime(env, JsHandle::RangeError, None, None, None)
    }
    
    pub fn new_uri(env: &mut JsEnv) -> JsError {
        Self::new_runtime(env, JsHandle::URIError, None, None, None)
    }
    
    pub fn new_reference(env: &mut JsEnv) -> JsError {
        Self::new_runtime(env, JsHandle::ReferenceError, None, None, None)
    }
    
    pub fn new_syntax(env: &mut JsEnv, message: &str) -> JsError {
        Self::new_runtime(env, JsHandle::SyntaxError, Some(message), None, None)
    }
    
    pub fn as_runtime(&self, env: &mut JsEnv) -> Root<JsValue> {
        match *self {
            JsError::Lex(ref message) | JsError::Parse(ref message) => {
                match Self::new_error(env, JsHandle::SyntaxError, Some(&message), None, None) {
                    Ok(error) => error,
                    Err(error) => error.as_runtime(env)
                }
            }
            JsError::Reference(ref message) => {
                match Self::new_error(env, JsHandle::ReferenceError, Some(&message), None, None) {
                    Ok(error) => error,
                    Err(error) => error.as_runtime(env)
                }
            }
            JsError::Runtime(ref error) => error.clone(),
            ref error @ _ => {
                // TODO #73: This could be nicer.
                let error = JsString::from_str(env, &format!("{:?}", error)).as_value(env);
                error.as_root(env)
            }
        }
    }
}

impl fmt::Debug for JsError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        try!(write!(formatter, "JsError {{ "));
        match *self {
            JsError::Io(ref err) => try!(err.fmt(formatter)),
            JsError::Lex(ref message) => try!(write!(formatter, "Lex {{ {} }}", message)),
            JsError::Parse(ref message) => try!(write!(formatter, "Parse {{ {} }}", message)),
            JsError::Reference(ref message) => try!(write!(formatter, "Reference {{ {} }}", message)),
            JsError::Runtime(..) => try!(write!(formatter, "Runtime {{ .. }}"))
        }
        write!(formatter, " }}")
    }
}

pub type JsResult<T> = Result<T, JsError>;
