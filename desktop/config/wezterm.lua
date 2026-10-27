local wezterm = require('wezterm')

local config = wezterm.config_builder()

local cargo_home = os.getenv('CARGO_HOME') or (os.getenv('HOME') .. '/.cargo')
config.default_prog = { cargo_home .. '/bin/wme' }

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

config.hide_tab_bar_if_only_one_tab = true
config.enable_scroll_bar = false
config.use_fancy_tab_bar = false

config.exit_behavior = 'CloseOnCleanExit'

config.audible_bell = 'Disabled'
config.visual_bell = { fade_in_duration_ms = 0, fade_out_duration_ms = 0 }

config.window_padding = { left = 10, right = 10, top = 8, bottom = 8 }
config.window_decorations = 'INTEGRATED_BUTTONS|RESIZE'
config.integrated_title_button_style = 'Windows'

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
