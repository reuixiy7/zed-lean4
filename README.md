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
- Paired abbreviations place the cursor between the delimiters: `\<>` → `⟨│⟩`, `\[[]]` → `⟦│⟧`, `\f<<>>` → `«│»`, and `\norm` → `‖│‖` (`│` marks the cursor). This also works when a space ends a partial abbreviation or Tab accepts a completion. A terminating space is kept after the closing delimiter. Other pairs, including `\floor`, `\ceil`, and `\{}`, also support cursor placement. Clients without snippet support insert plain text instead.
- Hover a Unicode symbol to see all abbreviations that produce it, such as `\R`, `\br`, `\bbR`, `\Real`, and `\real` for `ℝ`. Multi-character symbols such as `→ₗ` are matched as a whole. Input hints appear alongside Lean's documentation and any tactic state, and also work when Lean has no hover text. They follow the `abbreviations` setting; `infoview_hover` only controls the tactic state.
- Automatic abbreviation input requires the proxy (`"infoview": true`).
- For optional declaration and block templates, see [Optional snippets](#optional-snippets).

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

- The extension runs the Lean server through `lean-infoview-proxy`, a small Rust binary downloaded from GitHub releases. It follows the cursor and writes goals to `.lake/infoview.md` in the project.
- It shows the goals, the expected type, messages at the cursor and all messages of the file. Lines changed by the tactic under the cursor are marked with `-- ✱`.
- The infoview opens by itself the first time you open a Lean file in a project. After that Zed restores it with the rest of the layout. If you closed it, open `.lake/infoview.md` yourself.
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
| `infoview_file`       | `.lake/infoview.md` | where to write the infoview                                                     |
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

## Optional snippets

To add your own Lean templates, run `snippets: configure snippets` from Zed's command palette and open the Lean 4 scope (`lean 4.json`). Copy or merge the following into that local snippets file. These templates are optional and are not bundled with the extension. See [Zed's snippets documentation](https://zed.dev/docs/snippets) for customization.

```json
{
  "calc": {
    "prefix": "calc",
    "body": [
      "calc ${1:start} = ${2:first} := by ${3:sorry}",
      "  _ = ${4:second} := by ${5:sorry}"
    ],
    "description": "Scaffolding for authoring `calc` blocks"
  },
  "example": {
    "prefix": "example",
    "body": ["example : ${1:type} := ${2:term}"],
    "description": "An `example` declaration"
  },
  "namespace": {
    "prefix": "ns",
    "body": ["namespace ${1:name}", "", "end $1"],
    "description": "A new Lean namespace"
  },
  "section": {
    "prefix": "section",
    "body": ["section ${1:name}", "", "end $1"],
    "description": "A new Lean section"
  }
}
```

The `ns` prefix expands to a `namespace` block. Press Tab to move between placeholders; repeated placeholders such as `${1:name}` and `$1` keep the opening and closing names in sync.

## TODO list

- Tree-sitter-lean is experimental now and needs improvement
- Update and uninstall _elan_

## Development

To develop this extension, see the [Developing Extensions](https://zed.dev/docs/extensions/developing-extensions) section of the Zed docs.
