---
name: coderlm
description: "Codebase-aware exploration and code retrieval using Recursive Language Modeling. Connects to a coderlm-server that indexes the entire project (file tree, symbols, cross-references) and returns EXACT source code — full function implementations, variable lists, callers, line ranges — so you never need to guess, glob, grep, or read large files yourself. Replaces the typical scan-grep-read cycle with precise, index-backed lookups. Use when navigating unfamiliar codebases, finding functionality with nonobvious names, tracing cross-file dependencies, retrieving specific implementations, or when asked to \"explore\", \"understand\", or \"map out\" a codebase. Invoke with /coderlm."
allowed-tools:
  - Bash
  - Read
  - Task
---

# CoderLM — Recursive Language Model for Codebases

## The Problem This Solves

Codebases are hard to navigate. Functions have nonobvious names, functionality is spread across files, and cross-file linkages are buried. The default approach — glob for files, grep for keywords, read entire files into context — is wasteful. You guess at file names, scan irrelevant code, and burn context window on files that turn out to be unrelated.

CoderLM gives you a **complete index** of the project — every file, every symbol (function, class, struct, method), their signatures, callers, tests, and cross-references. **The server returns exact source code on demand**: full function bodies, variable declarations, precise line ranges. You never need to Read a 500-line file to find a 20-line function.

This is the Recursive Language Model pattern applied to code: you are the root LLM, the coderlm-server is your external environment, and the `coderlm-subcall` agent handles focused analysis when needed.

## What the Server Returns (Exact Code, Not Hints)

These commands return **the actual source code** — use them instead of the Read tool for targeted retrieval:

| Command | Returns |
|---------|---------|
| `impl function_name --file path` | The **complete source code** of the function/method/struct, extracted by tree-sitter. No need to Read the file. |
| `peek path --start N --end M` | **Exact lines N through M** of a file. Surgical reads without loading the whole file. |
| `variables function_name --file path` | Every local variable declared inside the function — names and the function they belong to. |
| `callers function_name --file path` | Every call site: file, line number, and the line of code containing the call. |
| `tests function_name --file path` | Every test referencing the symbol: name, file, line, and signature. |
| `grep "pattern"` | Every match across the indexed codebase: file, line, matched text, and surrounding context. |

**Key principle**: Prefer `impl` and `peek` over the Read tool. The server extracts exactly the code you need — a single function from a 1000-line file, a specific line range, callers across the entire project — without loading irrelevant code into context.

## Prerequisites

The `coderlm-server` must be running. It is a separate process:

```bash
coderlm-server serve            # indexes projects on-demand
coderlm-server serve /path/to/project  # pre-index a specific project
```

If the server is not running, all CLI commands will fail with a connection error.

## CLI

All interaction goes through the wrapper script. Full command reference is in `references/api-reference.md`.

```
python3 .claude/skills/coderlm/scripts/coderlm_cli.py <command> [args]
```

Abbreviated as `cli <command>` in examples below. The actual path is always:
```
python3 .claude/skills/coderlm/scripts/coderlm_cli.py
```

## Inputs

This skill reads `$ARGUMENTS`. Accepted patterns:
- `query=<question>` (required): what the user wants to understand or find
- `cwd=<path>` (optional): project directory, defaults to current working directory
- `port=<N>` (optional): server port, defaults to 3000

If the user did not supply a query, ask what they want to find or understand about the codebase.

## Workflow

### Step 1: Initialize

Create a session. The server indexes the project directory.

```bash
python3 .claude/skills/coderlm/scripts/coderlm_cli.py init
```

### Step 2: Orient

Get the project structure to understand the layout.

```bash
python3 .claude/skills/coderlm/scripts/coderlm_cli.py structure --depth 2
```

Read the output. Identify which directories likely contain code relevant to the query. Annotate important files as you go:

```bash
python3 .claude/skills/coderlm/scripts/coderlm_cli.py define-file src/server/mod.rs "HTTP server setup, route definitions"
```

### Step 3: Discover Symbols

Survey the symbol inventory to find functions, structs, and modules related to the query.

```bash
# Search by name
python3 .claude/skills/coderlm/scripts/coderlm_cli.py search "relevant_term" --limit 20

# List all functions in a specific file
python3 .claude/skills/coderlm/scripts/coderlm_cli.py symbols --kind function --file src/server/mod.rs

# Grep for patterns when symbol names are unknown
python3 .claude/skills/coderlm/scripts/coderlm_cli.py grep "error_pattern_or_keyword" --max-matches 20
```

This is the key step. The server knows every symbol in the project — use search and grep to find code you would not discover by browsing file names alone.

### Step 4: Retrieve Exact Code

Once you have found relevant symbols, **get their exact source code from the server** — do not use the Read tool:

```bash
# Get the full implementation of a function (returns the complete source)
python3 .claude/skills/coderlm/scripts/coderlm_cli.py impl function_name --file src/path.rs

# Get the exact variables declared inside a function
python3 .claude/skills/coderlm/scripts/coderlm_cli.py variables function_name --file src/path.rs

# Read a specific line range (surgical, not the whole file)
python3 .claude/skills/coderlm/scripts/coderlm_cli.py peek src/path.rs --start 40 --end 80
```

**Why this is better than Read**: `impl` returns just the function body, extracted by tree-sitter, even from files with hundreds of other functions. `peek` returns exactly the lines you specify. Neither loads irrelevant code into your context.

Only fall back to the Read tool when you genuinely need the entire file (e.g., understanding top-level imports, module structure, or when the file is small).

### Step 5: Trace Connections

Follow cross-file linkages to understand how code connects:

```bash
# Who calls this function? (returns file, line, and the actual calling code)
python3 .claude/skills/coderlm/scripts/coderlm_cli.py callers function_name --file src/path.rs

# What tests cover it? (returns test name, file, line, and signature)
python3 .claude/skills/coderlm/scripts/coderlm_cli.py tests function_name --file src/path.rs
```

This reveals the nonobvious linkages between files that are hard to find manually. The server searches the entire indexed codebase, not just the files you have already seen.

### Step 6: Delegate Focused Analysis

When you need to analyze a specific file or symbol in depth without loading it all into your context, delegate to the `coderlm-subcall` agent:

```
Use the coderlm-subcall agent with:
- File: src/complex_module.rs
- Query: "How does this module handle authentication errors?"
```

The subcall agent (Haiku) reads the code and returns structured JSON with findings, suggested next steps, and confidence levels. Use this for:
- Files you need analyzed but do not want to load into main context
- Parallel analysis of multiple files
- Quick relevance checks before committing to a deep dive

### Step 7: Annotate

As understanding grows, annotate files and symbols. Annotations are visible to all sessions on the same project — building shared knowledge.

```bash
python3 .claude/skills/coderlm/scripts/coderlm_cli.py define-symbol handle_request --file src/server/mod.rs "Routes incoming HTTP requests to the appropriate handler based on method and path"

python3 .claude/skills/coderlm/scripts/coderlm_cli.py mark tests/integration.rs test
```

### Step 8: Synthesize

Compile findings into a coherent answer. Reference specific files and line numbers. If the query requires code changes, you now know exactly which files and functions to modify.

### Step 9: Cleanup (optional)

```bash
python3 .claude/skills/coderlm/scripts/coderlm_cli.py cleanup
```

## Iteration

Steps 3-7 repeat as needed. A typical exploration:

1. Search for symbols related to the query
2. Retrieve the exact implementations with `impl`
3. Trace callers to understand usage patterns
4. Search for related symbols found in caller code
5. Annotate as understanding solidifies
6. Repeat until the query is answered

## When to Use the Server vs Native Tools

| Task | Use | Why |
|------|-----|-----|
| Find a function when you know the name | `search` | Instant index lookup vs globbing files |
| Find code when you do not know the name | `grep` + `symbols` | Searches all indexed files at once |
| Get a function's source code | `impl` | Returns just that function, even from large files |
| Read specific lines of a file | `peek` | Surgical line range, not the whole file |
| Understand what calls what | `callers` | Cross-project search, returns exact call sites |
| Find tests for a function | `tests` | Finds tests by symbol reference, not filename guessing |
| Get a project overview | `structure` | Tree with file counts and language breakdown |
| Find all functions in a module | `symbols --file` | Complete symbol inventory per file |
| Read an entire small file | Read tool | When you need all of it and it fits in context |

**Default to the server.** Use the Read tool only when you need an entire file or the server is unavailable.

## Guardrails

- **Use `impl` and `peek` instead of Read** for retrieving code — they return exactly what you need, nothing more.
- Use `search` and `grep` before reading files — discover first, then retrieve.
- Annotate as you go so the next query (or agent) benefits from your work.
- Check `history` to avoid redundant queries within a session.
- Quote specific code and file:line locations in your synthesis — do not summarize vaguely.
- The server is read-only. It never modifies the target codebase.

## Troubleshooting

### "Cannot connect to coderlm-server"
The server is not running. Start it:
```bash
coderlm-server serve
```

### "No active session"
Run `init` first:
```bash
python3 .claude/skills/coderlm/scripts/coderlm_cli.py init
```

### "Project was evicted"
The server evicted this project due to capacity limits (default: 5 projects). Re-initialize:
```bash
python3 .claude/skills/coderlm/scripts/coderlm_cli.py init
```

### Symbol search returns nothing relevant
Try broader grep patterns, or list all symbols to scan manually:
```bash
python3 .claude/skills/coderlm/scripts/coderlm_cli.py symbols --limit 200
python3 .claude/skills/coderlm/scripts/coderlm_cli.py grep "keyword" --context-lines 3
```
