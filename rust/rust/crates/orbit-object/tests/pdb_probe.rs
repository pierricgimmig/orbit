// Measures how far the `pdb` crate gets on Orbit's PDB test file, to decide
// whether stage 2g can reach exact parity with llvm::pdb.
#[test]
fn probe_dllmain_pdb() {
    let dir = std::env::var("ORBIT_TESTDATA").unwrap_or_else(|_| {
        format!("{}/../../../src/ObjectUtils/testdata", env!("CARGO_MANIFEST_DIR"))
    });
    let file = std::fs::File::open(format!("{dir}/dllmain.pdb")).unwrap();
    let mut pdb = pdb::PDB::open(file).unwrap();

    let info = pdb.pdb_information().unwrap();
    println!("guid={:?} age={}", info.guid, info.age);

    let dbi = pdb.debug_information().unwrap();
    let mut modules = dbi.modules().unwrap();
    let mut procs = 0usize;
    let mut samples: Vec<String> = Vec::new();
    while let Ok(Some(module)) = modules.next() {
        let Ok(Some(module_info)) = pdb.module_info(&module) else { continue };
        let Ok(symbols) = module_info.symbols() else { continue };
        let mut iter = symbols;
        while let Ok(Some(symbol)) = iter.next() {
            if let Ok(pdb::SymbolData::Procedure(proc)) = symbol.parse() {
                procs += 1;
                if samples.len() < 5 {
                    samples.push(format!("{} offset={:?} len={}", proc.name, proc.offset, proc.len));
                }
            }
        }
    }
    println!("procedure symbols: {procs}");
    for s in &samples { println!("  {s}"); }
}
