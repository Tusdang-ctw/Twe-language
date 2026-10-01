//! web3d-M5: the LLM primer's code is real Twe. The primer is what
//! every model writing Twe is given (the MCP guide, Studio, the
//! benchmark's system prompt), so an example in it that doesn't verify
//! teaches models to write broken programs. The benchmark's pilot found
//! three: an entity with `on update(dt):` (a parse error), top-level
//! key events, and examples using undefined names.

use twec::verify::verify_program;

/// The bodies of every ```` ```twe ```` block in `text`, with the line
/// each starts on.
fn twe_blocks(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines().enumerate();
    while let Some((n, line)) = lines.next() {
        if line.trim() == "```twe" {
            let body: Vec<&str> = lines.by_ref().map(|(_, l)| l).take_while(|l| l.trim() != "```").collect();
            out.push((n + 2, body.join("\n") + "\n"));
        }
    }
    out
}

#[test]
fn every_twe_block_in_the_primer_verifies() {
    let primer = twec::primer::guide();
    let blocks = twe_blocks(primer);
    assert!(blocks.len() >= 6, "found {} blocks", blocks.len());
    for (line, source) in blocks {
        let report = verify_program(&source);
        assert!(
            report.ok(),
            "the primer's block at docs/llm-primer.md:{line} doesn't verify:\n{source}\n{}",
            report.to_json()
        );
    }
}

#[test]
fn the_curated_examples_verify() {
    for ex in twec::primer::EXAMPLES {
        let report = verify_program(ex.source);
        assert!(report.ok(), "example `{}` doesn't verify: {}", ex.name, report.to_json());
    }
}
