# Async Request Queue Challenge

Today I noticed someone on Discord said:

> And I really want to see someone "vibe code" an async request queue.
> In Rust.
> Good luck lol

# Challenge Accepted

I wanted to see if I can implement it with as few prompts as possible, and without having to look at any code myself.

## Step 1: Research

I decided to just ask an AI ([perplexity.ai](perplexity.ai)), how I could achieve this task.

**Prompt 1:**

```
how can you implement an asynchronous request queue in rust with a single AI prompt with claude codex?
```

Result: [`docs/PERPLEXITY.md`](docs/PERPLEXITY.md) / [source](https://www.perplexity.ai/search/285f5250-0b70-40b0-9764-605ad0209198)


## Step 2: Project setup

Now that I had a plan, I created myself a new rust project using my [project templates](https://github.com/florianbuetow/ai-guardrails)

```
newrust async-request-queue-challenge
cd newrust async-request-queue-challenge
```

## Step 3: Implementation

For implementation I decided to use OpenAI's Codex because it's very good as instruction following.
I launched a new CLI session from the project directory

```
codex
```

And gave it the following prompt That contained the full perplexity search result, to start implementing an asynchronous request queue.

**Prompt 2:**

```
Lets build this thing, and use the justfile to run it and the checks until all of them pass. <PERPLEXITY RESULT>
```

(full prompt verbatim: [`docs/PROMPT.md`](docs/PROMPT.md))

After the implementation was done I told it to continue to work with the following prompt:

**Prompt 3:**
```
I don't want you to stop until all of the issues have been resolved please. And no more concerns remain. Make sure everything is tested in depth. In fact try to find ways to break it and then fix the code so it can't be broken.
```

You can review the full session log here: [`docs/SESSION_CODEX_IMPLEMENTATION.md`](docs/SESSION_CODEX_IMPLEMENTATION.md)

## Step 4: Adversarial Reviews

Codex was done. I decided to review the code using two different AIs.

For that I launched Claude code in two different teminals

```
claude
```

And gave them the following two prompts.

**Prompt 4:**

```
/codex:adversarial-review
```

**Prompt 5:**

```
please review the project for any race conditions or issues with the async request queue
```

You can see the full session logs for both of these sessions here: [`docs/SESSION_CODEX_ADVERSARIAL_REVIEW.md`](docs/SESSION_CODEX_ADVERSARIAL_REVIEW.md) and [`docs/SESSION_CLAUDE_ADVERSARIAL_REVIEW.md`](docs/SESSION_CLAUDE_ADVERSARIAL_REVIEW.md).


## Step 5: Feedback

With the review results I went back to the Codex implementation session and just pasted the findings and prompted it to fix the issues, improve the test until everything looked good.

**Prompt 6:**

```
I performed an adversarial review. I want you to cfreate tests to confirm the issues outlined in the review, and after your test confirm the issues exist, I want you to iterate to close all of the issues until none remain. You must not modify the tests to make them pass, you must fix the underlying issue in the code. If needed run multiple producers/consumers in parallel to show the issues in tests. All tests must be repeatable and become part of the test suite. Here are the findings and of the adversarial review:

<CODEX ADVERSARIAL REVIEW>

<CLAUDE ADVERSARIAL REVIEW>
```


# Conclusion

Total prompts used: **6**.

Transcripts are extracted by [`scripts/extract-sessions.sh`](scripts/extract-sessions.sh) — re-run it any time to refresh.

---

# Project Reference

## Repository Structure

```
async-request-queue-challenge/
├── Cargo.toml              # Project dependencies and metadata
├── rustfmt.toml            # Rustfmt configuration
├── deny.toml               # cargo-deny configuration (advisories, licenses, bans)
├── .pre-commit-config.yaml # Pre-commit hooks configuration
├── .gitignore              # Git ignore patterns
├── justfile                # Task runner with build/test/validation commands
├── AGENTS.md               # AI agent development rules
├── CLAUDE.md               # Claude Code compatibility (symlink to AGENTS.md)
├── README.md               # This file
├── src/                    # Source code
│   ├── main.rs             # Application entry point
│   └── lib.rs              # Library code and types
├── tests/                  # Integration tests
│   └── integration_test.rs # CLI integration tests
├── scripts/                # Utility scripts
├── data/                   # Data files
│   ├── input/             # Input data files
│   └── output/            # Generated output files
├── config/                 # Configuration files
│   ├── semgrep/           # Semgrep static analysis rules
│   │   ├── no-unwrap.yml
│   │   ├── no-expect-without-context.yml
│   │   ├── no-silent-error-discard.yml
│   │   ├── no-allow-attributes.yml
│   │   └── no-default-fallbacks.yml
│   └── codespell/         # Spell-check configuration
│       └── ignore.txt      # Spell-check ignore list
└── reports/                # Generated reports (not in git)
    └── coverage/          # Code coverage reports
```

## Prerequisites

- **Rust 1.85+** (2024 edition) - ([rustup.rs](https://rustup.rs/) or Homebrew)
- **just** - Command runner ([installation guide](https://github.com/casey/just#installation))
- **codespell** - Spell checker (`pip install codespell`)
- **semgrep** - Static analysis (`pip install semgrep`)

## Setup

Initialize the project environment:

```bash
just init
```

This will:
- Install Rust toolchain components (rustfmt, clippy, llvm-tools-preview)
- Install dev tools (cargo-nextest, cargo-deny, cargo-geiger, cargo-machete, grcov)
- Build the project

## Usage

Run the main application:

```bash
just run
```

See all available commands:

```bash
just help
```

Or simply:

```bash
just
```

## Development

### Available Commands

- `just init` - Initialize development environment
- `just run` - Run the main application
- `just destroy` - Remove build artifacts and reports
- `just help` - Show available commands

### Code Quality

- `just code-style` - Check code formatting (read-only)
- `just code-format` - Auto-fix code formatting
- `just code-typecheck` - Run cargo check + clippy
- `just code-security` - Run unsafe code detection (cargo-geiger)
- `just code-deptry` - Check dependency hygiene (cargo-machete)
- `just code-spell` - Check spelling
- `just code-audit` - Scan for vulnerabilities (cargo-deny)
- `just code-semgrep` - Run custom static analysis

### Testing

- `just test` - Run tests (cargo-nextest)
- `just test-coverage` - Run tests with coverage (grcov)

### CI

- `just ci` - Run all validation checks (verbose)
- `just ci-quiet` - Run all checks (silent, fail-fast)

The CI pipeline runs the following steps in order:
1. `init` - Install dependencies
2. `code-format` - Auto-format code
3. `code-style` - Verify formatting
4. `code-typecheck` - Type checking (cargo check + clippy)
5. `code-security` - Unsafe code scan (cargo-geiger)
6. `code-deptry` - Unused dependency check (cargo-machete)
7. `code-spell` - Spell checking (codespell)
8. `code-semgrep` - Custom static analysis (semgrep)
9. `code-audit` - Advisory/license/ban check (cargo-deny)
10. `test` - Tests (cargo-nextest)

## Project Rules

See [AGENTS.md](AGENTS.md) for detailed development guidelines including:
- Rust error handling rules (use `?` operator, never `.unwrap()`)
- Git commit guidelines (no AI attribution)
- Testing requirements
- Project structure conventions

## License

<!-- Add your license here -->
