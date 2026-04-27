use crate::value::{Env, RuntimeError, Value};

pub fn install(env: &mut Env) {
    env.set(
        "print".to_string(),
        Value::Builtin {
            name: "print",
            func: print_impl,
        },
    );
}

fn print_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    let parts: Vec<String> = args.iter().map(Value::display).collect();
    env.out.push_str(&parts.join(" "));
    env.out.push('\n');
    Ok(Value::Nil)
}
