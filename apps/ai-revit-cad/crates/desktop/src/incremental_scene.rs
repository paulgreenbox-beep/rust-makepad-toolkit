//! App integration of dirty-only derived geometry; Viewer remains a component.
use super::*;

impl App {
    /// Shared debug display mutation for the native button and geometry API.
    /// Opening visibility is derived display state; it never changes the document.
    pub(crate) fn set_show_opening_volumes(&mut self, cx: &mut Cx, show: bool) -> Result<Value, String> {
        let changed = self.show_opening_volumes != show || self.geometry_cache.show_opening_volumes() != show;
        self.show_opening_volumes = show;
        self.geometry_cache.set_show_opening_volumes(show);
        if self.document.is_some() && changed { self.refresh_scene(cx, false); }
        self.ui.widget(cx, ids!(show_opening_volumes)).set_text(cx, if show { "Объёмы проёмов: видны" } else { "Объёмы проёмов: скрыты" });
        Ok(json!({"show_opening_volumes":show,"changed":changed,"document_mutated":false}))
    }

    pub(super) fn refresh_scene_incremental(&mut self, cx: &mut Cx, fit: bool) {
        self.show_opening_volumes = self.geometry_cache.show_opening_volumes();
        let widget = self.viewport_widget(cx);
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let element_count = document.elements().len();
        let Some(mut viewport) = widget.borrow_mut::<CadViewport>() else {
            self.set_status(cx, "3D-вид ещё не готов");
            return;
        };
        self.last_rebuild = self
            .geometry_cache
            .rebuild_into(document, viewport.viewer_mut().scene_mut());
        let viewer = viewport.viewer_mut();
        viewer.camera_mut().up = Vec3::new(0.0, 0.0, 1.0);
        if fit {
            let _ = viewer.fit_scene(1.15);
        }
        let selected = self
            .selected_element
            .filter(|id| viewer.scene().get(id.get()).is_some())
            .map(ElementId::get);
        let _ = viewer.select(selected, false);
        viewport.redraw(cx);
        drop(viewport);
        let failure_count = self.geometry_cache.geometry_errors().count()
            + self.geometry_cache.resolution_errors().count();
        self.set_status(
            cx,
            &format!(
                "Модель: {element_count} · перестроено: {} · {} мс · ошибок: {failure_count}",
                self.last_rebuild.rebuilt_element_count, self.last_rebuild.rebuild_duration_ms
            ),
        );
        self.refresh_history(cx);
        self.refresh_inspector(cx);
        self.refresh_object_tree(cx);
    }
}
