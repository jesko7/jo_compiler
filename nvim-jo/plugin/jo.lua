-- Jo language support: treesitter highlighting + LSP (diagnostics + completions)

local plugin_root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")

-- -------------------------------------------------------------------------
-- 1. Filetype detection
-- -------------------------------------------------------------------------
vim.filetype.add({ extension = { jo = "jo" } })

-- -------------------------------------------------------------------------
-- 2. Tree-sitter parser
--    The compiled .so is bundled in grammar/jo.so and also copied into
--    nvim-treesitter's parser dir by install.sh.
-- -------------------------------------------------------------------------
do
  -- Load the bundled parser .so and register "jo" filetype → "jo" language.
  local so = plugin_root .. "/grammar/jo.so"
  if vim.fn.filereadable(so) == 1 then
    pcall(vim.treesitter.language.add, "jo", { path = so })
  end
  -- This is what makes vim.treesitter.language.get_lang("jo") return "jo",
  -- which is required by the global FileType * autocmd in treesitter.lua.
  pcall(vim.treesitter.language.register, "jo", "jo")

  -- Keep the queries dir in sync whenever the plugin loads.
  local qsrc = plugin_root .. "/queries/jo/highlights.scm"
  local qdst_dir = vim.fn.stdpath("data") .. "/lazy/nvim-treesitter/queries/jo"
  local qdst = qdst_dir .. "/highlights.scm"
  if vim.fn.isdirectory(qdst_dir) == 0 then vim.fn.mkdir(qdst_dir, "p") end
  if vim.fn.filereadable(qsrc) == 1 and
     (vim.fn.filereadable(qdst) == 0 or vim.fn.getftime(qsrc) > vim.fn.getftime(qdst)) then
    vim.fn.writefile(vim.fn.readfile(qsrc), qdst)
  end

  -- Register with nvim-treesitter so :TSInstall jo works too.
  local ok, parsers = pcall(require, "nvim-treesitter.parsers")
  if ok and parsers.get_parser_configs then
    parsers.get_parser_configs().jo = {
      install_info = {
        url = plugin_root .. "/grammar",
        files = { "src/parser.c" },
        generate_requires_npm = false,
        requires_generate_from_grammar = false,
      },
      filetype = "jo",
    }
  end
end

-- -------------------------------------------------------------------------
-- 3. Per-buffer setup (treesitter highlight + ftplugin options)
-- -------------------------------------------------------------------------
vim.api.nvim_create_autocmd("FileType", {
  group = vim.api.nvim_create_augroup("jo_buf", { clear = true }),
  pattern = "jo",
  callback = function(ev)
    -- Editor options
    vim.opt_local.tabstop     = 4
    vim.opt_local.shiftwidth  = 4
    vim.opt_local.expandtab   = true
    vim.opt_local.commentstring = "// %s"

    -- Treesitter highlighting
    local ok, err = pcall(vim.treesitter.start, ev.buf, "jo")
    if not ok then
      vim.notify("jo treesitter: " .. tostring(err), vim.log.levels.WARN)
    end
  end,
})

-- -------------------------------------------------------------------------
-- 4. LSP — register jo-lsp as a custom server
-- -------------------------------------------------------------------------
local lspconfig  = require("lspconfig")
local lsp_configs = require("lspconfig.configs")

if not lsp_configs.jo_lsp then
  lsp_configs.jo_lsp = {
    default_config = {
      cmd  = { "jo-lsp" },
      filetypes = { "jo" },
      root_dir  = function(fname)
        return require("lspconfig.util").find_git_ancestor(fname)
          or vim.fn.fnamemodify(fname, ":h")
      end,
      single_file_support = true,
    },
  }
end

local caps = vim.lsp.protocol.make_client_capabilities()
local ok_cmp, cmp_lsp = pcall(require, "cmp_nvim_lsp")
if ok_cmp then
  caps = vim.tbl_deep_extend("force", caps, cmp_lsp.default_capabilities())
end

lspconfig.jo_lsp.setup({
  capabilities = caps,
  on_attach = function(_, bufnr)
    local o = { buffer = bufnr, silent = true }
    vim.keymap.set("n", "gd",         vim.lsp.buf.definition,   o)
    vim.keymap.set("n", "K",          vim.lsp.buf.hover,         o)
    vim.keymap.set("n", "<leader>ca", vim.lsp.buf.code_action,   o)
    vim.keymap.set("n", "[d",         vim.diagnostic.goto_prev,  o)
    vim.keymap.set("n", "]d",         vim.diagnostic.goto_next,  o)
  end,
})
