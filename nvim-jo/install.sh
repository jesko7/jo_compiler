#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPILER_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== Building jo_compiler ==="
cd "$COMPILER_DIR"
cargo build --release
cp target/release/jo_compiler ~/.local/bin/jo_compiler

echo "=== Building jo-lsp ==="
cd "$SCRIPT_DIR/jo-lsp"
cargo build --release
cp target/release/jo-lsp ~/.local/bin/jo-lsp

echo "=== Building tree-sitter grammar ==="
cd "$SCRIPT_DIR/grammar"
tree-sitter generate
tree-sitter build
cp parser.so jo.so

# Install parser into nvim-treesitter
NVIM_DATA=$(nvim --headless --noplugin -c "lua io.write(vim.fn.stdpath('data'))" -c q 2>/dev/null)
mkdir -p "$NVIM_DATA/lazy/nvim-treesitter/parser"
cp jo.so "$NVIM_DATA/lazy/nvim-treesitter/parser/jo.so"

mkdir -p "$NVIM_DATA/lazy/nvim-treesitter/queries/jo"
cp "$SCRIPT_DIR/queries/jo/highlights.scm" "$NVIM_DATA/lazy/nvim-treesitter/queries/jo/highlights.scm"

echo "=== Done ==="
echo "Installed: jo_compiler, jo-lsp -> ~/.local/bin"
echo "Installed: tree-sitter parser  -> $NVIM_DATA/lazy/nvim-treesitter/parser/jo.so"
echo "Installed: highlights query    -> $NVIM_DATA/lazy/nvim-treesitter/queries/jo/"
