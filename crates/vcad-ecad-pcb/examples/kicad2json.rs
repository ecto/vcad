fn main() {
    let mut a = std::env::args().skip(1);
    let src = a.next().unwrap();
    let dst = a.next().unwrap();
    let pcb = vcad_ecad_symbols::parse_kicad_pcb(&std::fs::read_to_string(&src).unwrap()).unwrap();
    std::fs::write(&dst, serde_json::to_string(&pcb).unwrap()).unwrap();
}
