//! web3d-M6: the browser runtime imports nothing from `env`.
//!
//! macroquad / miniquad left bare `env` imports in the Twe web runtime,
//! which `web/env.js` provided through an import map. web3d-M6 removed
//! macroquad, so the runtime should import nothing from `env` — and
//! `web/env.js` + its import map are gone. This test re-parses the
//! built runtime's imports and fails if an `env` import comes back
//! (which would need that shim restored).
//!
//! Needs the runtime built first:
//! `cargo build -p twe-web --target wasm32-unknown-unknown --release`.
//! Without it the test prints a notice and passes, unless `TWE_WEB_WASM`
//! names the file to check — CI sets that, so there it is mandatory.

const DEFAULT_WASM: &str = "target/wasm32-unknown-unknown/release/twe_web.wasm";

#[test]
fn the_runtime_imports_nothing_from_env() {
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
    let env_imports: Vec<String> = wasm_imports(&bytes)
        .into_iter()
        .filter(|(module, _)| module == "env")
        .map(|(_, name)| name)
        .collect();
    assert!(
        env_imports.is_empty(),
        "the browser runtime imports from `env` again ({env_imports:?}); if macroquad          is back, restore web/env.js, the import map in web/index.html, and the env          entry in build::write_web_build"
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
