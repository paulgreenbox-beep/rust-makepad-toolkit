//! App-owned nonblocking close protocol for the circular compute worker.
//! WM_CLOSE and the Makepad caption button must both wait for actual reaping.
use super::*;

impl App {
    fn circular_begin_close(&mut self, cx: &mut Cx, window_id: WindowId) -> bool {
        self.circular_begin_shutdown();
        let waiting = !self.circular_is_quiescent();
        if waiting {
            self.circular_close_window = Some(window_id);
            self.set_status(cx, "Завершаю расчёт перед закрытием…");
            self.ui.redraw(cx);
            eprintln!(
                "CIRCULAR_LIFECYCLE {}",
                json!({
                    "event":"close_deferred", "window_index":window_id.id(),
                    "worker_quiescent":false
                })
            );
        }
        waiting
    }

    pub(super) fn circular_observe_close_request(
        &mut self,
        cx: &mut Cx,
        event: &WindowCloseRequestedEvent,
    ) {
        if self.ui.window(cx, ids!(main_window)).window_id() == Some(event.window_id)
            && self.circular_begin_close(cx, event.window_id)
        {
            event.accept_close.set(false);
        }
    }

    /// The toolkit caption button queues CloseWindow directly, bypassing WM_CLOSE.
    /// Inspect its actual semantic action before forwarding Actions to that Window.
    pub(super) fn circular_defer_caption_close(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        let clicked = self
            .ui
            .desktop_button(cx, ids!(main_window.windows_buttons.close))
            .clicked(actions)
            || self
                .ui
                .desktop_button(cx, ids!(main_window.caption_bar.windows_buttons.close))
                .clicked(actions);
        if !clicked {
            return false;
        }
        let Some(window_id) = self.ui.window(cx, ids!(main_window)).window_id() else {
            return false;
        };
        self.circular_begin_close(cx, window_id)
    }

    /// Called only after the existing timer has polled and reaped the worker.
    pub(super) fn circular_finish_deferred_close(&mut self, cx: &mut Cx) {
        if self.circular_close_window.is_none() || !self.circular_is_quiescent() {
            return;
        }
        let Some(window_id) = self.circular_close_window.take() else {
            return;
        };
        eprintln!(
            "CIRCULAR_LIFECYCLE {}",
            json!({
                "event":"close_after_reap", "window_index":window_id.id(),
                "worker_quiescent":true
            })
        );
        cx.push_unique_platform_op(CxOsOp::CloseWindow(window_id));
    }

    pub(super) fn circular_observe_shutdown(&mut self) {
        self.circular_begin_shutdown();
    }
}
