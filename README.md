# Zed Lean 4

A [Lean(4) Theorem Prover](https://lean-lang.org/) extension for [Zed](https://zed.dev).

- Tree-sitter: [tree-sitter-lean](https://github.com/Julian/tree-sitter-lean)
- Language Server: [Lean Language Server](https://github.com/leanprover/lean4/tree/master/src/Lean/Server)

## Lean Toolchain

The Lean Language Server is integrated with the Lean toolchain. It is recommended to manage Lean toolchain versions via [_elan_](https://github.com/leanprover/elan?tab=readme-ov-file#installation). For example, you can install the stable Lean version by running:

```sh
elan default stable
lean --version
```

## Features

### Highlight

- For optimal highlighting, set `"semantic_tokens"` to `"combined"`, which will enable LSP semantic tokens highlight in Zed.

### Abbreviation

- Abbreviations turn into symbols as you type: `\R` and a space gives `ℝ`, `\alpha` gives `α` right away. Turn off with `"abbreviations": false`.
- Ending a partial abbreviation chooses the shortest matching abbreviation: `\alp` followed by a space gives `α` and keeps the space. Other characters that cannot continue the abbreviation, including punctuation and newlines, also finish it and are kept. Exact matches take priority; equal-length matches use lexical order.
- To convert without adding whitespace, accept the Unicode completion suggestion with Tab using Zed's default key bindings. The proxy offers the same shortest match as a completion, such as `\alp` → `α` or `\R` → `ℝ`. If the suggestion is hidden, run `editor: show completions` first. When no completion is shown, Tab follows Zed's normal indentation/keymap behavior; the proxy cannot distinguish spaces inserted by Tab from manually inserted spaces.
- Paired abbreviations place the cursor between the delimiters: `\<>` → `⟨⟩`, `\[[]]` → `⟦⟧`, `\f<<>>` → `«»`, and `\norm` → `‖‖`. This also works when a space ends a partial abbreviation or Tab accepts a completion. A terminating space is kept after the closing delimiter. Other pairs, including `\floor`, `\ceil`, and `\{}`, also support cursor placement. Clients without snippet support insert plain text instead.
- Hover a Unicode symbol to see all abbreviations that produce it, such as `\R`, `\br`, `\bbR`, `\Real`, and `\real` for `ℝ`. Multi-character symbols such as `→ₗ` are matched as a whole. Input hints appear alongside Lean's documentation and any tactic state, and also work when Lean has no hover text. They follow the `abbreviations` setting; `infoview_hover` only controls the tactic state.
- To browse the complete abbreviation table, focus a Lean file, run `lsp command selector: toggle` from Zed's command palette, and select `Lean 4: Show Unicode Input Abbreviations` (no arguments needed). The proxy generates and opens `.lake/AbbreviationView.md` from its bundled mapping. The file contains only a table with abbreviations on the left and Unicode symbols on the right; `$CURSOR` marks the cursor position in paired delimiters, such as `{$CURSOR}`. Aliases remain grouped by symbol. If `infoview_file` is customized, the reference is saved beside it. This command requires Zed's LSP command selector and `window/showDocument` support, and remains available with `"abbreviations": false`.
- Unicode input, hover hints, and the abbreviation reference require the proxy (`"infoview": true`).

### Snippets

The extension includes thirteen templates with full keyword prefixes: `calc`, `example`, `namespace`, `section`, `variable`, `inductive`, `structure`, `class`, `theorem`, `def`, `instance`, `have`, and `match`. Type a keyword and select its snippet completion. Press Tab to move between placeholders; namespace and section names stay in sync with their matching `end` names. After entering a block name, the final cursor position is inside the block. These templates work independently of the proxy.

See [the bundled templates](snippets/lean%204.json). To add your own templates, run `snippets: configure snippets` from Zed's command palette and open the Lean 4 scope (`lean 4.json`). See [Zed's snippets documentation](https://zed.dev/docs/snippets) for customization.

### Declaration status

Like the gutter icons in VS Code, every declaration shows how it is doing:

```lean
theorem add_comm' (a b : ℕ) : a + b = b + a := by omega    ✅ Goals accomplished
theorem foo : 1 = 1 := by sorry                             ⚠️ declaration uses `sorry`
theorem bar : 1 = 2 := by simp                              ❌ unsolved goals ⊢ False
```

- Zed has no gutter icons for extensions, so the status is a diagnostic. Turn on inline diagnostics in the user `settings.json` to see it at the end of the line: `{ "diagnostics": { "inline": { "enabled": true } } }`. Without it, the status shows in the hover.
- ✅ is a hint on the name of a theorem or `Prop` `example` that Lean checked without errors or `sorry`. Like VS Code, it comes from Lean's silent "Goals accomplished!" message.
- `Try this:` suggestions (`simp?`, `exact?`) are put on one line, `💡 Try this: simp only [h]`, so the whole suggestion fits inline. Apply it with the code action (`ctrl-.` / `cmd-.`).
- The infoview counts proved theorems in its status line. Turn off with `"declaration_status": false`.

### Infoview

Zed extensions can't add panels, so the infoview is a Markdown file shown in Zed's Markdown preview.

- The extension runs the Lean server through `lean-infoview-proxy`, a small Rust binary downloaded from GitHub releases. It follows the cursor and writes goals to `.lake/InfoView.md` in the project.
- It shows the goals, the expected type, messages at the cursor and all messages of the file. Lines changed by the tactic under the cursor are marked with `-- ✱`.
- The infoview opens by itself the first time you open a Lean file in a project. After that Zed restores it with the rest of the layout. To open it manually, focus a Lean file, run `lsp command selector: toggle` from the command palette, and select `Lean 4: Open InfoView` (no arguments needed). This also works with `"infoview_auto_open": false` and opens the configured `infoview_file` without clearing the current goals. You can also open `.lake/InfoView.md` directly.
- Hover in a tactic block also shows the goals.
- Zed can't open it in a split by itself. Move it to the right once and Zed keeps the layout.

Recommended settings for the user `settings.json` (Zed doesn't allow them in project settings, so they apply to all Markdown files):

```json
{
  "markdown_preview": {
    "open_markdown_files_in_preview": true,
    "limit_content_width": false
  }
}
```

Settings in `lsp.lean4-lsp.settings`:

| Setting               | Default             |                                                                                 |
| --------------------- | ------------------- | ------------------------------------------------------------------------------- |
| `infoview`            | `true`              | `false` runs `lake serve` without the proxy                                     |
| `infoview_file`       | `.lake/InfoView.md` | where to write the infoview                                                     |
| `infoview_auto_open`  | `true`              | open the infoview the first time in a project                                   |
| `infoview_hover`      | `true`              | show goals in hover                                                             |
| `abbreviations`       | `true`              | Unicode input, completions, and abbreviation hints on hover                      |
| `declaration_status`  | `true`              | mark declarations ✅ / ⚠️ / ❌                                                  |
| `infoview_proxy_path` |                     | use a local build of the proxy (`cargo build -p lean-infoview-proxy --release`) |

### Nested Lean project

- If the Zed worktree is a parent directory of a Lean project, set `lsp.lean4-lsp.settings.lean_project_dir` to the project directory containing `lakefile.toml` (or `lakefile.lean`). Relative paths are resolved from the worktree root; absolute paths are also supported. This starts an automatically detected Lake with `--dir` so project dependencies such as Mathlib are available.
- A configured `lsp.lean4-lsp.binary` takes precedence over `lean_project_dir`. Omit `binary` to detect Lake automatically; with an explicit binary, include `"--dir", "/path/to/project"` before `"serve"` in `binary.arguments`.

### Installation

- Firstly, the extension will check `path` and `arguments` in `lsp.lean4-lsp.binary` settings. Then it will search `lake` in `$PATH`, try `$ELAN_HOME` and default directory.
- If Lean 4 is not detected, this extension can automatically install elan with default toolchain. To enable this feature, you must set `"elan_auto_install"` to `true`. You can also specify the default toolchain by setting `"elan_default_toolchain"` to `"stable"`, `"nightly"` or `"v4.28.0"`.
- During installation, the extension will automatically add `~/.elan` to your $PATH. The Lean toolchain can be completely removed by running `elan self uninstall`.

## settings.json Sample

```json
{
  "semantic_tokens": "combined",
  "lsp": {
    "lean4-lsp": {
      "binary": {
        "path": "/path/to/your/.elan/bin/lake",
        "arguments": [
          "--dir",
          "/path/to/your/worktree/formalization",
          "serve",
          "--"
        ]
      },
      "settings": {
        "elan_auto_install": true,
        "elan_default_toolchain": "stable",
        "lean_project_dir": "formalization"
      }
    }
  }
}
```

## TODO list

- Tree-sitter-lean is experimental now and needs improvement
- Update and uninstall _elan_

## Development

To develop this extension, see the [Developing Extensions](https://zed.dev/docs/extensions/developing-extensions) section of the Zed docs.
