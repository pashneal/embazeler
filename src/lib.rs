use nvim_oxi::{Dictionary, Function, Object, self, api::notify};



fn info(msg: String) { 
    let empty_opts = &Dictionary::new();
    notify(&msg, nvim_oxi::api::types::LogLevel::Info, empty_opts).expect("couldn't notify");
}

#[nvim_oxi::plugin]
fn embazeler() -> Dictionary {
    let add = Function::from_fn(|(a, b): (i32, i32)| a + b);

    let multiply = Function::from_fn(|(a, b): (i32, i32)| a * b);

    let compute = Function::from_fn(
        |(fun, a, b): (Function<(i32, i32), i32>, i32, i32)| {
            fun.call((a, b)).unwrap()
        },
    );

    let noft = Function::from_fn(info);

    let dict = Dictionary::from_iter([
        ("add", Object::from(add)),
        ("multiply", Object::from(multiply)),
        ("compute", Object::from(compute)),
        ("notify", Object::from(noft)),
    ]);
    return dict

}
