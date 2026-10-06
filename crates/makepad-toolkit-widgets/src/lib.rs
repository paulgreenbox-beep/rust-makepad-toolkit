//! Makepad rendering and event adapters for [`makepad_toolkit_core`].
//!
//! Domain adapters construct a pure [`DocumentFrame`]. [`VirtualCanvas`] owns the
//! Makepad draw/event lifecycle while delegating geometry and viewport state to core.

mod virtual_canvas;

pub use makepad_toolkit_core::*;
pub use makepad_widgets;
pub use virtual_canvas::*;

use makepad_widgets::{ScriptApply, ScriptMod, ScriptValue, ScriptVm, WidgetRegister};

makepad_widgets::script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.VirtualCanvas = set_type_default() do #(VirtualCanvas::register_widget(vm)) {
        width: Fill
        height: Fill
        draw_text.text_style: theme.font_regular
        draw_text_bold.text_style: theme.font_bold
        draw_text_italic.text_style: theme.font_italic
        draw_text_bold_italic.text_style: theme.font_bold_italic
        draw_text_mono.text_style: theme.font_code
        draw_text_mono_bold.text_style: theme.font_code
        draw_text_mono_italic.text_style: theme.font_code
        draw_text_mono_bold_italic.text_style: theme.font_code
    }
}
