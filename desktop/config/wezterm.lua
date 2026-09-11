-- wryme desktop shell
--
-- WezTerm configuration that makes the window look and behave like a small
-- calm desktop app rather than a terminal:
--   * the program that fills the window is `wme` (the wryme TUI)
--   * no tab bar, no scrollbar, no terminal chrome
--   * a quiet, flat color scheme
--   * the window closes when wryme exits cleanly
--
-- The launcher passes the path to this file via --config-file, so a user's
-- own WezTerm config is never touched.

local wezterm = require('wezterm')

local config = wezterm.config_builder()

-- Fill the window with wryme.
-- Use an absolute path derived from this file's own directory (wme always
-- lives beside wezterm.lua in the bundle's Resources/). "New Window" spawns
-- from default_prog and ignores the launcher's command line, so we must NOT
-- rely on `wme` being on the PATH — dock-launched apps get a minimal PATH
-- (e.g. /usr/bin:/bin:/usr/sbin:/sbin) that usually doesn't contain it.
local here = wezterm.config_dir
config.default_prog = { here .. '/wme' }

-- Quiet flat palette. Calm, low-contrast background for the TUI.
config.colors = {
    background = '#0b0b0f',
    foreground = '#d7d5db',
    ansi = {
        '#1c1c22',
        '#a8555c',
        '#6f9c6f',
        '#b08c4a',
        '#6f8fb0',
        '#9c7ba8',
        '#5fa0a0',
        '#d0ced6',
    },
    brights = {
        '#55555f',
        '#c96a72',
        '#8fc08f',
        '#d0a95a',
        '#8fb0d0',
        '#bc93ca',
        '#6fc0c0',
        '#f2f0f6',
    },
    cursor_fg = '#0b0b0f',
    cursor_bg = '#d7d5db',
}

-- No chrome: no tab bar (only ever one tab), no scrollbar.
config.hide_tab_bar_if_only_one_tab = true
config.enable_scroll_bar = false
config.use_fancy_tab_bar = false

-- Close the window when wryme exits cleanly instead of leaving a shell.
config.exit_behavior = 'CloseOnCleanExit'

-- Quiet: no bells.
config.audible_bell = 'Disabled'
config.visual_bell = { fade_in_duration_ms = 0, fade_out_duration_ms = 0 }

-- A bit of breathing room around the TUI. Keep the native resize border but
-- drop the title bar; window buttons are drawn into the (hidden) tab bar
-- area so the window still behaves like an app.
config.window_padding = { left = 10, right = 10, top = 8, bottom = 8 }
config.window_decorations = 'INTEGRATED_BUTTONS|RESIZE'
config.integrated_title_button_style = 'Windows'

-- Modest window size and a legible, modern font.
config.initial_cols = 108
config.initial_rows = 32
config.font_size = 13.0
config.font = wezterm.font_with_fallback({
    'JetBrains Mono',
    'Fira Code',
    'SF Mono',
    'Menlo',
    'Consolas',
})

return config
