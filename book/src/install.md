# Install + first program

## Build from source

Twe is a pure-Rust toolchain. You need [Rust 1.74+](https://rustup.rs).

```sh
git clone https://github.com/Tusdang-ctw/Twe-language
cd Twe-language
cargo build --release
./target/release/twec version
```

The binary is `target/release/twec` (or `twec.exe` on Windows).
Add the `target/release` directory to your `PATH` to run `twec`
from any folder.

Or install directly via cargo:

```sh
cargo install --git https://github.com/Tusdang-ctw/Twe-language --bin twec
```

Pre-built binaries land in
[Releases](https://github.com/Tusdang-ctw/Twe-language/releases)
once cargo-dist is wired up — see the
[release workflow](https://github.com/Tusdang-ctw/Twe-language/blob/main/.github/workflows/release.yml).

## Hello, Twe

Save this as `hello.twe`:

```twe
print("hello, twe!")
```

Run it headlessly:

```sh
twec run hello.twe
```

The `run` command lexes, parses, type-checks, and tree-walks the
program. Use `--frames N` to drive `on update(dt):` and
`on render():` for `N` frames in headless mode (useful for tests
and CI).

## Hello, game

Save this as `hello_game.twe`:

```twe
scene Hello:
    var time = 0.0

    initial: playing

    state playing:
        on update(dt):
            time += dt

        on render():
            rect(at: (300, 220),
                 size: (40, 40),
                 color: (math.sin(time * 2.0) * 0.5 + 0.5,
                         0.6,
                         0.9,
                         1.0))
            text("hello, game!",
                 at: (240, 280),
                 size: 24,
                 color: color.white)
```

Run it in the macroquad window:

```sh
twec play hello_game.twe
```

Hot reload is on — edit the file while the window is open and the
script re-runs automatically.

## Hello, 3D

```sh
twec play3d examples/hello_3d.twe
```

Then try the showcase:

```sh
twec play3d examples/crystal_hunter.twe
```

## What's next

- [Tutorial](./tutorial.md) walks you through three real games
  (Pong, Survivors, RPG) end to end.
- [Examples gallery](./examples.md) lists every runnable example
  and what it shows.
- [Reference](./reference.md) is the formal stdlib + grammar.
