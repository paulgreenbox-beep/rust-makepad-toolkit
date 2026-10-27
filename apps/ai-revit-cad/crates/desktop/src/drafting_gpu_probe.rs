//! GPU diagnostics capability boundary for the public Makepad SDK.
//! Diagnostic requests do not modify, manually submit or replace SDK rendering.
//! Live device/texture access is unavailable, so native timings remain unknown.
use drafting_core::CachedFrame;
use makepad_widgets::Cx;
use serde_json::{Value, json};

#[cfg(target_os = "windows")]
mod windows {
    use super::*;

    const UNSUPPORTED: &str = "UNSUPPORTED_PUBLIC_SDK: live D3D11 device and texture diagnostics are unavailable in the public Makepad SDK";

    #[derive(Default)]
    pub struct Diagnostics;
    impl Diagnostics {
        pub fn poll(&mut self) {}
        pub fn has_pending_upload(&mut self) -> bool {
            false
        }
        pub fn start_upload(&mut self, _: &Cx, _: &CachedFrame) -> Result<Value, String> {
            Err(UNSUPPORTED.into())
        }
        pub fn start_fence(&mut self, _: &Cx, _: u64, _: String) -> Result<Value, String> {
            Err(UNSUPPORTED.into())
        }
        pub fn upload_state(&mut self, _: &str) -> Result<Value, String> {
            Err(UNSUPPORTED.into())
        }
        pub fn fence_state(&mut self, _: &str) -> Result<Value, String> {
            Err(UNSUPPORTED.into())
        }
    }

    fn unsupported_profile(metadata: Value) -> Value {
        json!({
            "status": "unsupported",
            "measurement": "actual_native_canvas_pass",
            "metadata": metadata,
            "actual_instance_resource_write_bytes": null,
            "actual_instance_resource_write_null_reason": "UNSUPPORTED_PUBLIC_SDK",
            "cpu_instance_resource_update_ms": null,
            "canvas_pass_gpu_ms": null,
            "actual_adapter": null,
            "gpu_qualification": "NOT_RUN",
            "renderer_modified": false,
            "normal_renderer_fallback_safe": true,
            "validity": {"gpu_timestamps": false},
            "null_reason": "UNSUPPORTED_PUBLIC_SDK",
            "error": {"code": "UNSUPPORTED_PUBLIC_SDK", "message": UNSUPPORTED}
        })
    }

    #[derive(Default)]
    pub struct NativeProfiler {
        latest: Option<Value>,
    }
    impl NativeProfiler {
        pub fn profile(
            &mut self,
            _: &mut Cx,
            _: makepad_widgets::Area,
            _: makepad_widgets::Area,
            _: &[makepad_widgets::Area],
            metadata: Value,
        ) -> Value {
            // In particular, do not consume paint_dirty or submit a pass here.
            let value = unsupported_profile(metadata);
            self.latest = Some(value.clone());
            value
        }
        pub fn poll(&mut self) -> Value {
            self.latest
                .clone()
                .unwrap_or_else(|| unsupported_profile(Value::Null))
        }
    }

    // These source-level traversal/layout guards and their original assertions
    // remain testable without a native device. They do not qualify GPU runtime.
    #[cfg(test)]
    mod retained_guard_tests {
        use super::*;
        use makepad_widgets::{DrawListId, DrawPassId};
        use std::collections::HashSet;
        const MAX_DRAW_TREE_ITEMS: usize = 100_000;

        // Both the list and its children must be renderer-ready before we consume its
        // paint_dirty flag manually. A retained child list may have an older redraw id.
        fn ready_list(
            cx: &Cx,
            id: DrawListId,
            pass_id: DrawPassId,
            active: &mut HashSet<DrawListId>,
            remaining: &mut usize,
            depth: usize,
        ) -> Result<(), String> {
            if depth > 64 {
                return Err("DRAW_TREE_DEPTH_LIMIT".into());
            }
            if cx.draw_lists.is_id_freed(id) {
                return Err("DRAW_LIST_FREED".into());
            }
            let list = cx.draw_lists.checked_index(id).ok_or("DRAW_LIST_STALE")?;
            if list.draw_pass_id != Some(pass_id) {
                return Err("DRAW_LIST_FOREIGN_PASS".into());
            }
            if !active.insert(id) {
                return Err("DRAW_TREE_CYCLE".into());
            }
            for order in 0..list.draw_item_order_len() {
                if *remaining == 0 {
                    return Err("DRAW_TREE_ITEM_LIMIT".into());
                }
                *remaining -= 1;
                let index = list
                    .draw_item_id_at_order_index(order)
                    .ok_or("DRAW_ITEM_STALE")?;
                if index >= list.draw_items.len() {
                    return Err("DRAW_ITEM_STALE".into());
                }
                let item = &list.draw_items[index];
                if let Some(child) = item.kind.sub_list() {
                    ready_list(cx, child, pass_id, active, remaining, depth + 1)?;
                    continue;
                }
                let Some(call) = item.kind.draw_call() else {
                    continue;
                };
                let instances = item.instances.as_ref().ok_or("CPU_BATCH_NOT_FINISHED")?;
                if instances.is_empty() {
                    continue;
                }
                let shader = cx
                    .draw_shaders
                    .shaders
                    .get(call.draw_shader_id.index)
                    .ok_or("DRAW_SHADER_STALE")?;
                if !cx.is_draw_shader_window_ready(call.draw_shader_id) {
                    return Err("DRAW_SHADER_NOT_READY".into());
                }
                if shader.mapping.uses_time {
                    return Err("TIME_SHADER_UNSUPPORTED".into());
                }
                let slots = shader.mapping.instances.total_slots;
                if slots == 0 || instances.len() % slots != 0 {
                    return Err("INSTANCE_LAYOUT_INVALID".into());
                }
                if call.geometry_id.is_none() {
                    return Err("DRAW_GEOMETRY_UNAVAILABLE".into());
                }
            }
            active.remove(&id);
            Ok(())
        }

        // Search active draw order, not retained storage. Unrelated parent widgets
        // need no shader/pass readiness check: they are outside the measured pass.
        // Every list on the actual quad's path must still belong to its parent pass.
        struct CompositorPath {
            active: HashSet<DrawListId>,
            path: Vec<(DrawListId, usize)>,
            found: Option<Vec<(DrawListId, usize)>>,
            remaining: usize,
        }

        impl CompositorPath {
            fn visit(
                &mut self,
                cx: &Cx,
                id: DrawListId,
                parent_id: DrawPassId,
                target: (DrawListId, usize),
                ancestors_owned: bool,
            ) -> Result<(), String> {
                if self.path.len() > 64 {
                    return Err("COMPOSITOR_DRAW_TREE_DEPTH_LIMIT".into());
                }
                if cx.draw_lists.is_id_freed(id) {
                    return Err("COMPOSITOR_DRAW_LIST_FREED".into());
                }
                let list = cx
                    .draw_lists
                    .checked_index(id)
                    .ok_or("COMPOSITOR_DRAW_LIST_STALE")?;
                if !self.active.insert(id) {
                    return Err("COMPOSITOR_DRAW_TREE_CYCLE".into());
                }
                let owned = ancestors_owned && list.draw_pass_id == Some(parent_id);
                for order in 0..list.draw_item_order_len() {
                    if self.remaining == 0 {
                        return Err("COMPOSITOR_DRAW_TREE_ITEM_LIMIT".into());
                    }
                    self.remaining -= 1;
                    let index = list
                        .draw_item_id_at_order_index(order)
                        .ok_or("COMPOSITOR_DRAW_ITEM_STALE")?;
                    self.path.push((id, order));
                    if (id, index) == target {
                        if !owned {
                            return Err("COMPOSITOR_TARGET_PATH_FOREIGN_PASS".into());
                        }
                        if self.found.is_some() {
                            return Err("COMPOSITOR_TARGET_PATH_AMBIGUOUS".into());
                        }
                        self.found = Some(self.path.clone());
                    }
                    if let Some(child) = list.draw_items[index].kind.sub_list() {
                        self.visit(cx, child, parent_id, target, owned)?;
                    }
                    self.path.pop();
                }
                self.active.remove(&id);
                Ok(())
            }
        }

        fn compositor_path(
            cx: &Cx,
            root: DrawListId,
            parent_id: DrawPassId,
            target: (DrawListId, usize),
        ) -> Result<Vec<(DrawListId, usize)>, String> {
            let mut search = CompositorPath {
                active: HashSet::new(),
                path: Vec::new(),
                found: None,
                remaining: MAX_DRAW_TREE_ITEMS,
            };
            search.visit(cx, root, parent_id, target, true)?;
            search
                .found
                .ok_or_else(|| "COMPOSITOR_TARGET_NOT_REACHABLE".into())
        }

        fn compositor_instance_range(
            offset: usize,
            count: usize,
            slots: usize,
            payload_len: usize,
        ) -> Result<(), String> {
            let end = count
                .checked_mul(slots)
                .and_then(|span| offset.checked_add(span))
                .ok_or("COMPOSITOR_INSTANCE_RANGE_OVERFLOW")?;
            if slots == 0
                || count != 1
                || payload_len == 0
                || payload_len % slots != 0
                || offset % slots != 0
                || end > payload_len
            {
                return Err("COMPOSITOR_INSTANCE_RANGE_INVALID".into());
            }
            Ok(())
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            use makepad_widgets::{DrawList, DrawPass};

            struct Fixture {
                // Handles own the pool slots: retain them through assertions, then
                // drop them before Cx (fields drop in declaration order).
                retained_passes: Vec<DrawPass>,
                retained_lists: Vec<DrawList>,
                cx: Cx,
            }

            impl Fixture {
                fn new() -> Self {
                    Self {
                        retained_passes: Vec::new(),
                        retained_lists: Vec::new(),
                        cx: Cx::new(Box::new(|_, _| {})),
                    }
                }

                fn pass(&mut self) -> DrawPassId {
                    let pass = DrawPass::new(&mut self.cx);
                    let id = pass.draw_pass_id();
                    self.retained_passes.push(pass);
                    id
                }

                fn list(&mut self, pass_id: DrawPassId) -> DrawListId {
                    let list = DrawList::new(&mut self.cx);
                    let id = list.id();
                    self.cx.draw_lists[id].draw_pass_id = Some(pass_id);
                    self.retained_lists.push(list);
                    id
                }

                fn append(&mut self, parent: DrawListId, child: DrawListId) {
                    let redraw_id = self.cx.redraw_id;
                    self.cx.draw_lists[parent].append_sub_list(redraw_id, child);
                }
            }

            #[test]
            fn finds_target_through_nested_active_draw_order() {
                let mut fixture = Fixture::new();
                let parent_pass = fixture.pass();
                let root = fixture.list(parent_pass);
                let middle = fixture.list(parent_pass);
                let target = fixture.list(parent_pass);
                let leaf = fixture.list(parent_pass);
                fixture.cx.passes[parent_pass].main_draw_list_id = Some(root);
                fixture.append(root, middle);
                fixture.append(middle, target);
                fixture.append(target, leaf);

                let path = compositor_path(&fixture.cx, root, parent_pass, (target, 0)).unwrap();
                assert_eq!(path, vec![(root, 0), (middle, 0), (target, 0)]);
            }

            #[test]
            fn unrelated_foreign_ui_is_allowed_but_foreign_target_ancestor_is_rejected() {
                let mut fixture = Fixture::new();
                let parent_pass = fixture.pass();
                let foreign_pass = fixture.pass();
                let root = fixture.list(parent_pass);
                let foreign_ui = fixture.list(foreign_pass);
                let foreign_target = fixture.list(parent_pass);
                let foreign_leaf = fixture.list(parent_pass);
                let canvas_ui = fixture.list(parent_pass);
                let canvas_target = fixture.list(parent_pass);
                let canvas_leaf = fixture.list(parent_pass);
                fixture.cx.passes[parent_pass].main_draw_list_id = Some(root);
                fixture.append(root, foreign_ui);
                fixture.append(foreign_ui, foreign_target);
                fixture.append(foreign_target, foreign_leaf);
                fixture.append(root, canvas_ui);
                fixture.append(canvas_ui, canvas_target);
                fixture.append(canvas_target, canvas_leaf);

                assert!(
                    compositor_path(&fixture.cx, root, parent_pass, (canvas_target, 0)).is_ok()
                );
                assert_eq!(
                    compositor_path(&fixture.cx, root, parent_pass, (foreign_target, 0))
                        .unwrap_err(),
                    "COMPOSITOR_TARGET_PATH_FOREIGN_PASS"
                );
            }

            #[test]
            fn rejects_missing_and_reordered_out_of_draw_order_targets() {
                let mut fixture = Fixture::new();
                let parent_pass = fixture.pass();
                let root = fixture.list(parent_pass);
                let detached = fixture.list(parent_pass);
                let detached_leaf = fixture.list(parent_pass);
                fixture.append(detached, detached_leaf);
                assert_eq!(
                    compositor_path(&fixture.cx, root, parent_pass, (detached, 0)).unwrap_err(),
                    "COMPOSITOR_TARGET_NOT_REACHABLE"
                );

                let root = fixture.list(parent_pass);
                let target_branch = fixture.list(parent_pass);
                let target = fixture.list(parent_pass);
                let target_leaf = fixture.list(parent_pass);
                let visible_branch = fixture.list(parent_pass);
                let visible_leaf = fixture.list(parent_pass);
                fixture.append(root, target_branch);
                fixture.append(target_branch, target);
                fixture.append(target, target_leaf);
                fixture.append(root, visible_branch);
                fixture.append(visible_branch, visible_leaf);
                fixture.cx.draw_lists[root].draw_item_reorder = Some(vec![1]);

                assert_eq!(
                    compositor_path(&fixture.cx, root, parent_pass, (target, 0)).unwrap_err(),
                    "COMPOSITOR_TARGET_NOT_REACHABLE"
                );
            }

            #[test]
            fn rejects_duplicate_target_occurrences() {
                let mut fixture = Fixture::new();
                let parent_pass = fixture.pass();
                let root = fixture.list(parent_pass);
                let middle = fixture.list(parent_pass);
                let target_leaf = fixture.list(parent_pass);
                fixture.append(root, middle);
                fixture.append(root, middle);
                fixture.append(middle, target_leaf);

                assert_eq!(
                    compositor_path(&fixture.cx, root, parent_pass, (middle, 0)).unwrap_err(),
                    "COMPOSITOR_TARGET_PATH_AMBIGUOUS"
                );
            }

            #[test]
            fn rejects_cycles_in_parent_compositor_tree() {
                let mut fixture = Fixture::new();
                let parent_pass = fixture.pass();
                let root = fixture.list(parent_pass);
                let branch = fixture.list(parent_pass);
                fixture.append(root, branch);
                fixture.append(branch, root);

                assert_eq!(
                    compositor_path(&fixture.cx, root, parent_pass, (branch, 0)).unwrap_err(),
                    "COMPOSITOR_DRAW_TREE_CYCLE"
                );
            }

            #[test]
            fn canvas_readiness_still_rejects_a_foreign_pass_list() {
                let mut fixture = Fixture::new();
                let canvas_pass = fixture.pass();
                let foreign_pass = fixture.pass();
                let foreign_list = fixture.list(foreign_pass);
                let mut remaining = 8;

                assert_eq!(
                    ready_list(
                        &fixture.cx,
                        foreign_list,
                        canvas_pass,
                        &mut HashSet::new(),
                        &mut remaining,
                        0,
                    )
                    .unwrap_err(),
                    "DRAW_LIST_FOREIGN_PASS"
                );
            }

            #[test]
            fn compositor_instance_range_checks_layout_alignment_and_overflow() {
                assert!(compositor_instance_range(9, 1, 9, 18).is_ok());
                assert_eq!(
                    compositor_instance_range(1, 1, 9, 18).unwrap_err(),
                    "COMPOSITOR_INSTANCE_RANGE_INVALID"
                );
                assert_eq!(
                    compositor_instance_range(0, 2, 9, 18).unwrap_err(),
                    "COMPOSITOR_INSTANCE_RANGE_INVALID"
                );
                assert_eq!(
                    compositor_instance_range(9, 1, 9, 9).unwrap_err(),
                    "COMPOSITOR_INSTANCE_RANGE_INVALID"
                );
                assert_eq!(
                    compositor_instance_range(usize::MAX, 2, usize::MAX, usize::MAX).unwrap_err(),
                    "COMPOSITOR_INSTANCE_RANGE_OVERFLOW"
                );
            }
        }
    }
}
#[cfg(target_os = "windows")]
pub use windows::{Diagnostics, NativeProfiler};

#[cfg(not(target_os = "windows"))]
#[derive(Default)]
pub struct Diagnostics;
#[cfg(not(target_os = "windows"))]
impl Diagnostics {
    pub fn poll(&mut self) {}
    pub fn has_pending_upload(&mut self) -> bool {
        false
    }
    pub fn start_upload(&mut self, _: &Cx, _: &CachedFrame) -> Result<Value, String> {
        Err("GPU_PROBE_UNSUPPORTED_PLATFORM".into())
    }
    pub fn start_fence(&mut self, _: &Cx, _: u64, _: String) -> Result<Value, String> {
        Err("GPU_FENCE_UNSUPPORTED_PLATFORM".into())
    }
    pub fn upload_state(&mut self, _: &str) -> Result<Value, String> {
        Err("GPU_PROBE_UNSUPPORTED_PLATFORM".into())
    }
    pub fn fence_state(&mut self, _: &str) -> Result<Value, String> {
        Err("GPU_FENCE_UNSUPPORTED_PLATFORM".into())
    }
}

#[cfg(not(target_os = "windows"))]
#[derive(Default)]
pub struct NativeProfiler {
    latest: Option<Value>,
}
#[cfg(not(target_os = "windows"))]
impl NativeProfiler {
    pub fn profile(
        &mut self,
        _: &mut Cx,
        _: makepad_widgets::Area,
        _: makepad_widgets::Area,
        _: &[makepad_widgets::Area],
        metadata: Value,
    ) -> Value {
        let value = json!({"status":"partial","measurement":"actual_native_canvas_pass","metadata":metadata,
            "actual_instance_resource_write_bytes":null,
            "actual_instance_resource_write_null_reason":"GPU_BACKEND_UNSUPPORTED",
            "cpu_instance_resource_update_ms":null,"canvas_pass_gpu_ms":null,
            "normal_renderer_fallback_safe":true,
            "null_reason":"GPU_PROFILER_UNSUPPORTED_PLATFORM"});
        self.latest = Some(value.clone());
        value
    }
    pub fn poll(&mut self) -> Value {
        self.latest.clone().unwrap_or_else(|| {
            json!({"status":"unavailable",
            "measurement":"actual_native_canvas_pass","canvas_pass_gpu_ms":null,
            "null_reason":"NO_CANVAS_PROFILE_SAMPLE"})
        })
    }
}
