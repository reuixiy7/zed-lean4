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
- The same abbreviations are also [snippets](https://github.com/owlx56/zed-lean4/blob/main/snippets/lean%204.json). Use the snippet for brackets like `\<>` if you want the cursor inside.

### Infoview

Zed extensions can't add panels, so the infoview is a Markdown file shown in Zed's Markdown preview.

- The extension runs the Lean server through `lean-infoview-proxy`, a small Rust binary downloaded from GitHub releases. It follows the cursor and writes goals to `.lake/infoview.md` in the project.
- The file opens when you open a Lean file. It shows the goals, the expected type, messages at the cursor and all messages of the file. Lines changed by the tactic under the cursor are marked with `-- ✱`.
- Hover in a tactic block also shows the goals.
- Zed can't open it in a split by itself. Move it to the right once and Zed keeps the layout.

Recommended Zed settings (better in the Lean project's `.zed/settings.json`, since they apply to all Markdown files):

```json
{ "markdown_preview": { "open_markdown_files_in_preview": true, "limit_content_width": false } }
```

Settings in `lsp.lean4-lsp.settings`:

| Setting | Default | |
|---|---|---|
| `infoview` | `true` | `false` runs `lake serve` without the proxy |
| `infoview_file` | `.lake/infoview.md` | where to write the infoview |
| `infoview_auto_open` | `true` | open the infoview with the first Lean file |
| `infoview_hover` | `true` | show goals in hover |
| `abbreviations` | `true` | replace abbreviations as you type |
| `infoview_proxy_path` | | use a local build of the proxy (`cargo build -p lean-infoview-proxy --release`) |

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
        "arguments": ["--dir", "/path/to/your/worktree/formalization", "serve", "--"]
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
