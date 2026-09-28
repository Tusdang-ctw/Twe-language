//! web3d-M2: the browser runtime's `env` imports are all provided.
//!
//! macroquad / miniquad (the 2D backend, linked until web3d-M6) leave
//! bare `env` imports in the Twe web runtime. `web/index.html` maps
//! `env` to `web/env.js` with an import map; if the runtime ever needs an
//! import `env.js` doesn't export, the page fails to instantiate with a
//! link error — before any Twe code runs. This test reads the import
//! section of the built runtime and checks every `env` import against
//! the functions `env.js` exports.
//!
//! Needs the runtime built first:
//! `cargo build -p twe-web --target wasm32-unknown-unknown --release`.
//! Without it the test prints a notice and passes, unless `TWE_WEB_WASM`
//! names the file to check — CI sets that, so there it is mandatory.

use std::collections::BTreeSet;

const DEFAULT_WASM: &str = "target/wasm32-unknown-unknown/release/twe_web.wasm";

#[test]
fn env_js_provides_every_env_import() {
    let (path, required) = match std::env::var("TWE_WEB_WASM") {
        Ok(p) => (p, true),
        Err(_) => (DEFAULT_WASM.to_string(), false),
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if !required => {
            eprintln!("skipping web runtime import check: {path}: {e}");
            return;
        }
        Err(e) => panic!("TWE_WEB_WASM={path}: {e}"),
    };
    let needed: BTreeSet<String> = wasm_imports(&bytes)
        .into_iter()
        .filter(|(module, _)| module == "env")
        .map(|(_, name)| name)
        .collect();
    assert!(
        !needed.is_empty(),
        "no `env` imports found in {path}: if macroquad is gone (web3d-M6), \
         delete web/env.js, its import map and this test"
    );
    let env_js = std::fs::read_to_string("web/env.js").expect("read web/env.js");
    let provided: BTreeSet<String> = env_js
        .lines()
        .filter_map(|l| l.trim().strip_prefix("export function "))
        .filter_map(|rest| rest.split('(').next())
        .map(|name| name.trim().to_string())
        .collect();
    let missing: Vec<_> = needed.difference(&provided).collect();
    assert!(
        missing.is_empty(),
        "web/env.js is missing exports the runtime imports from `env`: {missing:?}"
    );
}

/// `(module, name)` for every import in a wasm binary.
fn wasm_imports(wasm: &[u8]) -> Vec<(String, String)> {
    assert_eq!(&wasm[..4], b"\0asm", "not a wasm binary");
    let mut r = Reader { b: wasm, at: 8 };
    while r.at < wasm.len() {
        let id = r.byte();
        let size = r.leb() as usize;
        let end = r.at + size;
        if id == 2 {
            let mut out = Vec::new();
            for _ in 0..r.leb() {
                let module = r.name();
                let name = r.name();
                match r.byte() {
                    0 => {
                        r.leb(); // function: type index
                    }
                    1 => {
                        r.byte(); // table: element type, then limits
                        r.limits();
                    }
                    2 => r.limits(), // memory
                    3 => {
                        r.byte(); // global: value type, mutability
                        r.byte();
                    }
                    4 => {
                        r.byte(); // tag: attribute, type index
                        r.leb();
                    }
                    k => panic!("unknown import kind {k}"),
                }
                out.push((module, name));
            }
            return out;
        }
        r.at = end;
    }
    Vec::new()
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> u8 {
        let v = self.b[self.at];
        self.at += 1;
        v
    }

    fn leb(&mut self) -> u64 {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let b = self.byte();
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return v;
            }
            shift += 7;
        }
    }

    fn name(&mut self) -> String {
        let len = self.leb() as usize;
        let s = String::from_utf8_lossy(&self.b[self.at..self.at + len]).into_owned();
        self.at += len;
        s
    }

    fn limits(&mut self) {
        let flags = self.byte();
        self.leb();
        if flags & 1 != 0 {
            self.leb();
        }
    }
}
