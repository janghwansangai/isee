use tract_onnx::prelude::*;
fn main() -> TractResult<()> {
    for p in std::env::args().skip(1) {
        let m = tract_onnx::onnx().model_for_path(&p)?;
        println!("== {p}");
        for (i, o) in m.input_outlets()?.iter().enumerate() {
            println!("  in {i}: {} {:?}", m.node(o.node).name, m.outlet_fact(*o)?);
        }
        for (i, o) in m.output_outlets()?.iter().enumerate() {
            println!("  out {i}: {} {:?}", m.node(o.node).name, m.outlet_fact(*o)?);
        }
    }
    Ok(())
}
