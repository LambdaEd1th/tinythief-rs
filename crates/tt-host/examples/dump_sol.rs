//! Dump the contents of a Flash shared object (.sol) file.
//!
//!   cargo run -p tt-host --example dump_sol -- saves/TinyThief.swf/tinyThiefDataNew.sol

use flash_lso::read::Reader;
use flash_lso::types::{Lso, Value};

fn show(value: &Value, out: &mut String) {
    match value {
        Value::Bool(b) => out.push_str(&b.to_string()),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::Integer(i) => out.push_str(&i.to_string()),
        Value::String(s) => out.push_str(&format!("{s:?}")),
        Value::Null => out.push_str("null"),
        Value::Undefined => out.push_str("undefined"),
        Value::StrictArray(_, items) | Value::ECMAArray(_, items, _, _) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                show(v, out);
            }
            out.push(']');
        }
        Value::Object(_, elements, _) => {
            out.push('{');
            for (i, e) in elements.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{}: ", e.name));
                show(e.value(), out);
            }
            out.push('}');
        }
        other => out.push_str(&format!("{other:?}")),
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_sol <file.sol>");
    let bytes = std::fs::read(&path).expect("read file");
    let lso: Lso = Reader::default().parse(&bytes).expect("parse lso");
    println!("header: {} v{}", lso.header.name, lso.header.format_version);
    for element in &lso.body {
        let mut out = String::new();
        show(element.value(), &mut out);
        println!("{:<28} {}", element.name, out);
    }
}
