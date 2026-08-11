//! Print a corpus case's spec as JSON (debug aid).
fn main() {
    let id = std::env::args().nth(1).expect("case id");
    let corpus = vcad_torture::build_corpus();
    let c = corpus.iter().find(|c| c.id == id).expect("unknown id");
    println!("{}", serde_json::to_string_pretty(c).unwrap());
}
