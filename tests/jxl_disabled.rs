//! WS2: with the `jxl` feature disabled, `.jxl` inputs produce a clear
//! per-file error instead of a generic decode failure.

#![cfg(not(feature = "jxl"))]

use byteshaver::input::load_source;

#[test]
fn jxl_input_errors_when_feature_disabled() {
    let path = std::env::temp_dir().join(format!(
        "byteshaver-jxl-disabled-{}.jxl",
        std::process::id()
    ));
    std::fs::write(&path, b"this is not a real jxl file").expect("write fixture");

    let error = match load_source(&path) {
        Err(error) => error,
        Ok(_) => panic!(".jxl input must fail without the jxl feature"),
    };
    let message = error.to_string();
    assert!(
        message.contains("not compiled in") && message.contains("jxl"),
        "{message}"
    );

    let _ = std::fs::remove_file(&path);
}
