"""web3d-M5: the Python + pygame-ce baseline's grader and checker.

`twec bench` drives this; it is not meant to be run by hand.

    python harness.py grade   < {"program", "ticks", "commands", "checks"}
    python harness.py check   < {"program"}

grade: runs the program's `Game` class headless (SDL's dummy video and
audio drivers) for `ticks` fixed 60 Hz ticks. Each tick it replays one
input command, as compiled by twec from the task's input script (so both
languages get exactly the same input), calls `game.update(dt, events)`,
then `game.draw(screen)`. After a check's tick, its Python expression is
evaluated with `game` in scope. Prints one JSON grade in the same shape
as `twec bench grade --json`.

check: the baseline's counterpart of `twec verify`: `compile()` for
syntax errors, then pyflakes. Prints {"errors": n, "report": text}.

Security: `grade` executes model-written Python (exec) and the task's
check expressions (eval) by design: running the program is what is
being measured. Unlike a Twe program, a Python program can touch the
whole machine, so run benchmark sessions on models you trust, or inside
a container or VM (bench/README.md). The checks come from the task
files in this repository, not from the model.

Input reaches the program the ways pygame offers:
- pygame.key.get_pressed()[pygame.K_d]  held keys
- pygame.key.get_just_pressed()          keys pressed this tick
- pygame.event.get()                      this tick's KEYDOWN / KEYUP /
                                          MOUSEBUTTONDOWN / MOUSEBUTTONUP
                                          events (also passed to update)
- pygame.mouse.get_pos(), get_pressed()
"""

import io
import json
import math
import os
import sys
import traceback

os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
os.environ.setdefault("SDL_AUDIODRIVER", "dummy")
os.environ.setdefault("PYGAME_HIDE_SUPPORT_PROMPT", "1")

STDOUT_CAP = 16 * 1024
DT = 1.0 / 60.0


def key_code(pygame, name):
    special = {
        "space": pygame.K_SPACE, "enter": pygame.K_RETURN, "escape": pygame.K_ESCAPE,
        "up": pygame.K_UP, "down": pygame.K_DOWN, "left": pygame.K_LEFT, "right": pygame.K_RIGHT,
        "tab": pygame.K_TAB, "backspace": pygame.K_BACKSPACE, "shift": pygame.K_LSHIFT,
        "ctrl": pygame.K_LCTRL, "alt": pygame.K_LALT,
    }
    if name in special:
        return special[name]
    return pygame.key.key_code(name)


MOUSE_BUTTONS = {"left": 1, "middle": 2, "right": 3}


class Keys:
    """What pygame.key.get_pressed() returns: indexable by key code."""

    def __init__(self, codes):
        self.codes = codes

    def __getitem__(self, code):
        return code in self.codes

    def __len__(self):
        return 512


def failed(stage, error, checks):
    return {
        "passed": False, "stage": stage, "error": error, "ticks_run": 0,
        "checks": [{"name": c["name"], "passed": False, "detail": "not reached"} for c in checks],
        "stdout": "",
    }


def short_error(exc):
    """The exception and the program line it came from."""
    tb = traceback.extract_tb(exc.__traceback__)
    where = ""
    for frame in reversed(tb):
        if frame.filename == "<program>":
            where = "line %d: " % frame.lineno
            break
    return "%s%s: %s" % (where, type(exc).__name__, exc)


def grade(job):
    program, ticks, commands, checks = job["program"], job["ticks"], job["commands"], job["checks"]
    out = io.StringIO()
    real_stdout = sys.stdout
    sys.stdout = out
    try:
        import pygame
        pygame.init()
        screen = pygame.display.set_mode((640, 480))
        try:
            code = compile(program, "<program>", "exec")
        except SyntaxError as e:
            return failed("parse", "line %s: %s" % (e.lineno, e.msg), checks)
        namespace = {"__name__": "__main__program__"}
        # Input, replaced each tick.
        state = {"held": set(), "pressed": set(), "mouse": (0, 0), "mb": set(), "events": []}

        def get_events(*_args, **_kwargs):
            events, state["events"] = state["events"], []
            return events

        pygame.key.get_pressed = lambda: Keys(state["held"])
        pygame.key.get_just_pressed = lambda: Keys(state["pressed"])
        pygame.mouse.get_pos = lambda: state["mouse"]
        pygame.mouse.get_pressed = lambda num_buttons=3: tuple(b in state["mb"] for b in (1, 2, 3))[:num_buttons]
        pygame.event.get = get_events
        pygame.event.pump = lambda: None
        try:
            exec(code, namespace)
            game_class = namespace.get("Game")
            if game_class is None:
                return failed("load", "the program defines no class `Game`", checks)
            game = game_class()
        except Exception as e:  # noqa: BLE001 — any failure is the program's
            return failed("load", short_error(e), checks)

        results = [None] * len(checks)
        env = {"game": game, "math": math, "abs": abs, "len": len, "min": min, "max": max,
               "round": round, "all": all, "any": any, "sum": sum, "Vector3": pygame.Vector3}

        def run_checks(tick):
            for i, c in enumerate(checks):
                if c["at"] != tick:
                    continue
                try:
                    v = eval(c["expr"], dict(env))
                    results[i] = {"name": c["name"], "passed": v is True or v == True,  # noqa: E712
                                  "detail": repr(v)}
                except Exception as e:  # noqa: BLE001
                    results[i] = {"name": c["name"], "passed": False, "detail": "%s: %s" % (type(e).__name__, e)}

        run_checks(0)
        error = None
        ticks_run = 0
        prev_held = set()
        prev_mb = set()
        for tick in range(ticks):
            cmd = commands[tick]
            held = {key_code(pygame, k) for k in cmd["held"]}
            pressed = {key_code(pygame, k) for k in cmd["pressed"]}
            mb = {MOUSE_BUTTONS[b] for b in cmd["mb_held"] if b in MOUSE_BUTTONS}
            mb_press = {MOUSE_BUTTONS[b] for b in cmd["mb_press"] if b in MOUSE_BUTTONS}
            mouse = (int(cmd["mouse"][0]), int(cmd["mouse"][1]))
            events = []
            for k in sorted(prev_held - held):
                events.append(pygame.event.Event(pygame.KEYUP, key=k))
            for k in sorted(pressed):
                events.append(pygame.event.Event(pygame.KEYDOWN, key=k, unicode="", mod=0))
            for b in sorted(prev_mb - mb):
                events.append(pygame.event.Event(pygame.MOUSEBUTTONUP, button=b, pos=mouse))
            for b in sorted(mb_press):
                events.append(pygame.event.Event(pygame.MOUSEBUTTONDOWN, button=b, pos=mouse))
            state.update(held=held, pressed=pressed, mouse=mouse, mb=mb, events=list(events))
            prev_held, prev_mb = held, mb
            try:
                game.update(DT, events)
                game.draw(screen)
            except Exception as e:  # noqa: BLE001
                error = "tick %d: %s" % (tick, short_error(e))
                break
            if out.tell() > STDOUT_CAP * 2:
                text = out.getvalue()[:STDOUT_CAP]
                out.seek(0)
                out.truncate()
                out.write(text)
            ticks_run = tick + 1
            run_checks(ticks_run)
        checks_out = [r or {"name": c["name"], "passed": False, "detail": "not reached"}
                      for r, c in zip(results, checks)]
        return {
            "passed": error is None and all(c["passed"] for c in checks_out),
            "stage": "run" if error else "checks",
            "error": error,
            "ticks_run": ticks_run,
            "checks": checks_out,
            "stdout": out.getvalue()[:STDOUT_CAP],
        }
    finally:
        sys.stdout = real_stdout


ERROR_MESSAGES = {
    "UndefinedName", "UndefinedLocal", "UndefinedExport", "DuplicateArgument",
    "ReturnOutsideFunction", "YieldOutsideFunction", "ContinueOutsideLoop", "BreakOutsideLoop",
    "MultiValueRepeatedKeyLiteral", "TooManyExpressionsInStarredAssignment", "TwoStarredExpressions",
    "PercentFormatInvalidFormat", "StringDotFormatInvalidFormat", "ForwardAnnotationSyntaxError",
}


def check(job):
    program = job["program"]
    try:
        compile(program, "<program>", "exec")
    except SyntaxError as e:
        return {"errors": 1, "report": "line %s: SyntaxError: %s" % (e.lineno, e.msg)}
    from pyflakes import api, reporter

    class Collect(reporter.Reporter):
        def __init__(self):
            self.messages = []

        def unexpectedError(self, filename, msg):
            self.messages.append(("error", "error: %s" % msg))

        def syntaxError(self, filename, msg, lineno, offset, text):
            self.messages.append(("error", "line %s: SyntaxError: %s" % (lineno, msg)))

        def flake(self, message):
            kind = "error" if type(message).__name__ in ERROR_MESSAGES else "warning"
            self.messages.append((kind, "line %d: %s" % (message.lineno, message.message % message.message_args)))

    r = Collect()
    api.check(program, "<program>", r)
    errors = sum(1 for k, _ in r.messages if k == "error")
    report = "\n".join("%s %s" % (k, m) for k, m in r.messages)
    return {"errors": errors, "report": report}


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    job = json.loads(sys.stdin.buffer.read().decode("utf-8"))
    if mode == "grade":
        result = grade(job)
    elif mode == "check":
        result = check(job)
    else:
        print("usage: harness.py grade|check < job.json", file=sys.stderr)
        sys.exit(2)
    sys.stdout.write(json.dumps(result))
    sys.stdout.flush()


if __name__ == "__main__":
    main()
