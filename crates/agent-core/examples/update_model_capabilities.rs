use std::{env, path::Path};

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let input = std::fs::read_to_string(&args[0]).expect("read downloaded catalog");
    let document = agent_core::parse_model_capability_document(
        serde_json::from_str(&input).expect("catalog JSON"),
    )
    .expect("recent model records");
    agent_core::write_model_capabilities(Path::new(&args[1]), &document).expect("write snapshot");
    println!("Saved {} model mappings", document.models.len());
}
