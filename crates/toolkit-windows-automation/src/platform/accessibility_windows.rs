//! Native Microsoft UI Automation provider for a Makepad HWND.
//!
//! The provider is deliberately driven by an application-neutral snapshot. It
//! subclasses the Makepad window only to answer WM_GETOBJECT; drawing and widget
//! lifecycle remain owned by Makepad.

use crate::accessibility::{
    AccessibilityIndex, AccessibilityRole, AccessibilityTree, AccessibilityTreeError,
    IndexedAccessibilityNode, MAKEPAD_EMPTY_VALUE_SENTINEL,
};
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::mem::{ManuallyDrop, transmute};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, VARIANT_BOOL, VARIANT_TRUE, WPARAM};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_ARRAY, VT_BOOL, VT_BSTR, VT_I4,
    VT_R8, VT_UNKNOWN,
};
use windows::Win32::UI::Accessibility::{
    IInvokeProvider, IInvokeProvider_Impl, IRawElementProviderFragment,
    IRawElementProviderFragment_Impl, IRawElementProviderFragmentRoot,
    IRawElementProviderFragmentRoot_Impl, IRawElementProviderSimple,
    IRawElementProviderSimple_Impl, ISelectionItemProvider, ISelectionItemProvider_Impl,
    ISelectionProvider, ISelectionProvider_Impl, IValueProvider, IValueProvider_Impl,
    NavigateDirection, NavigateDirection_FirstChild, NavigateDirection_LastChild,
    NavigateDirection_NextSibling, NavigateDirection_Parent, NavigateDirection_PreviousSibling,
    ProviderOptions, ProviderOptions_ServerSideProvider, UIA_AutomationIdPropertyId,
    UIA_BoundingRectanglePropertyId, UIA_ClassNamePropertyId, UIA_ControlTypePropertyId,
    UIA_FrameworkIdPropertyId, UIA_HasKeyboardFocusPropertyId, UIA_InvokePatternId,
    UIA_IsContentElementPropertyId, UIA_IsControlElementPropertyId, UIA_IsEnabledPropertyId,
    UIA_IsKeyboardFocusablePropertyId, UIA_IsOffscreenPropertyId,
    UIA_LocalizedControlTypePropertyId, UIA_NamePropertyId, UIA_ProviderDescriptionPropertyId,
    UIA_RuntimeIdPropertyId, UIA_SelectionCanSelectMultiplePropertyId,
    UIA_SelectionIsSelectionRequiredPropertyId, UIA_SelectionItemIsSelectedPropertyId,
    UIA_SelectionItemPatternId, UIA_SelectionPatternId, UIA_ValuePatternId,
    UIA_ValueValuePropertyId, UiaAppendRuntimeId, UiaHostProviderFromHwnd, UiaRect,
    UiaReturnRawElementProvider, UiaRootObjectId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GWLP_WNDPROC, GetWindowLongPtrW, IsWindow, SetWindowLongPtrW,
    WM_GETOBJECT, WM_NCDESTROY, WNDPROC,
};
use windows::core::{
    BOOL, BSTR, Error, HRESULT, IUnknown, Interface, PCWSTR, Result as WinResult, implement,
};

const E_FAIL: HRESULT = HRESULT(0x8000_4005u32 as i32);
const UIA_E_ELEMENTNOTAVAILABLE: HRESULT = HRESULT(0x8004_0201u32 as i32);
const UIA_E_NOTSUPPORTED: HRESULT = HRESULT(0x8004_0204u32 as i32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccessibilityAction {
    Invoke,
    Select,
    AddToSelection,
    RemoveFromSelection,
    Focus,
    SetValue(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessibilityActionRequest {
    pub node_id: String,
    pub action: AccessibilityAction,
}

pub type AccessibilityActionHandler =
    Arc<dyn Fn(AccessibilityActionRequest) -> Result<(), String> + Send + Sync + 'static>;

struct BridgeState {
    hwnd_value: isize,
    old_wnd_proc: isize,
    snapshot: RwLock<AccessibilityIndex>,
    action_handler: AccessibilityActionHandler,
}

static BRIDGES: OnceLock<Mutex<HashMap<isize, Arc<BridgeState>>>> = OnceLock::new();

fn bridges() -> &'static Mutex<HashMap<isize, Arc<BridgeState>>> {
    BRIDGES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A generic Windows UIA bridge attached to an existing Makepad window.
pub struct WindowsAccessibilityBridge {
    hwnd_value: isize,
    state: Arc<BridgeState>,
}

impl WindowsAccessibilityBridge {
    pub fn attach(
        hwnd: HWND,
        tree: AccessibilityTree,
        action_handler: AccessibilityActionHandler,
    ) -> WinResult<Self> {
        let hwnd_value = hwnd.0 as isize;
        if hwnd.0.is_null() || !unsafe { IsWindow(Some(hwnd)).as_bool() } {
            return Err(bridge_error(
                E_FAIL,
                "cannot attach accessibility bridge to an invalid HWND",
            ));
        }
        let index = tree
            .index()
            .map_err(|error| bridge_error(E_FAIL, error.to_string()))?;

        let mut registry = bridges()
            .lock()
            .expect("accessibility bridge registry poisoned");
        if registry.contains_key(&hwnd_value) {
            return Err(bridge_error(
                E_FAIL,
                "an accessibility bridge is already attached to this HWND",
            ));
        }

        let old_wnd_proc = unsafe {
            SetWindowLongPtrW(
                hwnd,
                GWLP_WNDPROC,
                accessibility_wnd_proc as *const () as usize as isize,
            )
        };
        if old_wnd_proc == 0 {
            return Err(Error::from_win32());
        }

        let state = Arc::new(BridgeState {
            hwnd_value,
            old_wnd_proc,
            snapshot: RwLock::new(index),
            action_handler,
        });
        registry.insert(hwnd_value, state.clone());
        Ok(Self { hwnd_value, state })
    }

    pub fn hwnd(&self) -> HWND {
        hwnd_from_value(self.hwnd_value)
    }

    pub fn update(&self, tree: AccessibilityTree) -> Result<(), AccessibilityTreeError> {
        let index = tree.index()?;
        *self
            .state
            .snapshot
            .write()
            .expect("accessibility snapshot lock poisoned") = index;
        Ok(())
    }

    pub fn generation(&self) -> u64 {
        self.state
            .snapshot
            .read()
            .expect("accessibility snapshot lock poisoned")
            .generation
    }
}

impl Drop for WindowsAccessibilityBridge {
    fn drop(&mut self) {
        let removed = bridges()
            .lock()
            .expect("accessibility bridge registry poisoned")
            .remove(&self.hwnd_value);
        if removed.is_none() {
            return;
        }
        let hwnd = hwnd_from_value(self.hwnd_value);
        if !hwnd.0.is_null() && unsafe { IsWindow(Some(hwnd)).as_bool() } {
            let current = unsafe { GetWindowLongPtrW(hwnd, GWLP_WNDPROC) };
            if current == accessibility_wnd_proc as *const () as usize as isize {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_WNDPROC, self.state.old_wnd_proc);
                }
            }
        }
    }
}

unsafe extern "system" fn accessibility_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state = bridges()
        .lock()
        .ok()
        .and_then(|registry| registry.get(&(hwnd.0 as isize)).cloned());

    let Some(state) = state else {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    };

    if message == WM_GETOBJECT && lparam.0 as i32 == UiaRootObjectId {
        let root_id = state
            .snapshot
            .read()
            .ok()
            .map(|snapshot| snapshot.root_id.clone());
        if let Some(root_id) = root_id {
            let provider: IRawElementProviderSimple = Provider::new(state, root_id).into();
            return unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, &provider) };
        }
    }

    let result = call_previous_wnd_proc(state.old_wnd_proc, hwnd, message, wparam, lparam);
    if message == WM_NCDESTROY {
        let _ = bridges()
            .lock()
            .map(|mut registry| registry.remove(&(hwnd.0 as isize)));
    }
    result
}

fn call_previous_wnd_proc(
    old_wnd_proc: isize,
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if old_wnd_proc == 0 {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    } else {
        let old: WNDPROC = unsafe { transmute(old_wnd_proc) };
        unsafe { CallWindowProcW(old, hwnd, message, wparam, lparam) }
    }
}

#[derive(Clone)]
#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider,
    ISelectionItemProvider,
    ISelectionProvider,
    IValueProvider
)]
struct Provider {
    state: Arc<BridgeState>,
    node_id: String,
    /// Only transient controls opt in to this token. It prevents a late UIA
    /// invoke from reaching a replacement control with the same AutomationId.
    instance_key: Option<u64>,
}

impl Provider {
    fn new(state: Arc<BridgeState>, node_id: String) -> Self {
        let instance_key = state.snapshot.read().ok().and_then(|snapshot| {
            snapshot
                .nodes
                .get(&node_id)
                .and_then(|node| node.node.instance_key)
        });
        Self {
            state,
            node_id,
            instance_key,
        }
    }

    fn current_snapshot(&self) -> std::sync::RwLockReadGuard<'_, AccessibilityIndex> {
        self.state
            .snapshot
            .read()
            .expect("accessibility snapshot lock poisoned")
    }

    fn node(&self) -> WinResult<IndexedAccessibilityNode> {
        let node = self
            .current_snapshot()
            .nodes
            .get(&self.node_id)
            .cloned()
            .ok_or_else(|| {
                bridge_error(UIA_E_ELEMENTNOTAVAILABLE, "accessibility node is stale")
            })?;
        if self.instance_key.is_some() && node.node.instance_key != self.instance_key {
            return Err(bridge_error(
                UIA_E_ELEMENTNOTAVAILABLE,
                "accessibility provider targets a replaced transient control",
            ));
        }
        Ok(node)
    }

    fn simple_for(&self, node_id: String) -> IRawElementProviderSimple {
        Provider::new(self.state.clone(), node_id).into()
    }

    fn fragment_for(&self, node_id: String) -> IRawElementProviderFragment {
        Provider::new(self.state.clone(), node_id).into()
    }

    fn is_root(&self) -> bool {
        self.current_snapshot().root_id == self.node_id
    }

    fn action(&self, action: AccessibilityAction) -> WinResult<()> {
        let node = self.node()?.node;
        let allowed = match action {
            AccessibilityAction::Invoke => node.actions.invoke,
            AccessibilityAction::Select => node.actions.select,
            AccessibilityAction::AddToSelection => node.actions.add_to_selection,
            AccessibilityAction::RemoveFromSelection => node.actions.remove_from_selection,
            AccessibilityAction::Focus => node.actions.focus,
            AccessibilityAction::SetValue(_) => node.actions.set_value,
        };
        if !allowed {
            return Err(bridge_error(
                UIA_E_NOTSUPPORTED,
                "accessibility action is not supported",
            ));
        }
        (self.state.action_handler)(AccessibilityActionRequest {
            node_id: self.node_id.clone(),
            action,
        })
        .map_err(|error| bridge_error(E_FAIL, error))
    }

    fn selection_container_id(&self) -> WinResult<String> {
        let snapshot = self.current_snapshot();
        let mut parent = snapshot
            .nodes
            .get(&self.node_id)
            .and_then(|node| node.parent_id.clone());
        while let Some(parent_id) = parent {
            let parent_node = snapshot.nodes.get(&parent_id).ok_or_else(|| {
                bridge_error(UIA_E_ELEMENTNOTAVAILABLE, "selection parent is stale")
            })?;
            if parent_node.node.selection.is_some() {
                return Ok(parent_id);
            }
            parent = parent_node.parent_id.clone();
        }
        Err(bridge_error(
            UIA_E_NOTSUPPORTED,
            "selection item has no selection container",
        ))
    }

    fn navigation_target(&self, direction: NavigateDirection) -> WinResult<Option<String>> {
        let snapshot = self.current_snapshot();
        let node = snapshot.nodes.get(&self.node_id).ok_or_else(|| {
            bridge_error(UIA_E_ELEMENTNOTAVAILABLE, "accessibility node is stale")
        })?;

        let target = if direction == NavigateDirection_Parent {
            node.parent_id.clone()
        } else if direction == NavigateDirection_FirstChild {
            node.children.first().cloned()
        } else if direction == NavigateDirection_LastChild {
            node.children.last().cloned()
        } else if direction == NavigateDirection_NextSibling
            || direction == NavigateDirection_PreviousSibling
        {
            let Some(parent_id) = node.parent_id.as_deref() else {
                return Ok(None);
            };
            let siblings = &snapshot
                .nodes
                .get(parent_id)
                .ok_or_else(|| {
                    bridge_error(UIA_E_ELEMENTNOTAVAILABLE, "accessibility parent is stale")
                })?
                .children;
            let position = siblings.iter().position(|id| id == &self.node_id);
            position.and_then(|position| {
                if direction == NavigateDirection_NextSibling {
                    siblings.get(position + 1).cloned()
                } else {
                    position
                        .checked_sub(1)
                        .and_then(|index| siblings.get(index).cloned())
                }
            })
        } else {
            None
        };
        Ok(target)
    }

    fn hit_test(&self, x: f64, y: f64) -> WinResult<String> {
        let snapshot = self.current_snapshot();
        let root_id = snapshot.root_id.clone();
        hit_test_index(&snapshot, &root_id, x, y).ok_or_else(|| {
            bridge_error(UIA_E_ELEMENTNOTAVAILABLE, "no accessibility node at point")
        })
    }

    fn focused_node(&self) -> WinResult<String> {
        let snapshot = self.current_snapshot();
        Ok(snapshot
            .nodes
            .values()
            .find(|node| node.node.focused)
            .map(|node| node.node.id.clone())
            .unwrap_or_else(|| snapshot.root_id.clone()))
    }
}

fn hit_test_index(snapshot: &AccessibilityIndex, node_id: &str, x: f64, y: f64) -> Option<String> {
    let node = snapshot.nodes.get(node_id)?;
    if !node.node.visible || !contains(node.node.bounds, x, y) {
        return None;
    }
    for child_id in node.children.iter().rev() {
        if let Some(hit) = hit_test_index(snapshot, child_id, x, y) {
            return Some(hit);
        }
    }
    Some(node_id.to_owned())
}

fn contains(bounds: crate::AccessibilityRect, x: f64, y: f64) -> bool {
    x >= bounds.x && y >= bounds.y && x <= bounds.x + bounds.width && y <= bounds.y + bounds.height
}

#[allow(non_upper_case_globals)]
impl IRawElementProviderSimple_Impl for Provider_Impl {
    fn ProviderOptions(&self) -> WinResult<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(
        &self,
        patternid: windows::Win32::UI::Accessibility::UIA_PATTERN_ID,
    ) -> WinResult<IUnknown> {
        let node = self.node()?.node;
        if patternid == UIA_InvokePatternId && node.actions.invoke {
            return Ok(IInvokeProvider::from(self.this.clone()).into());
        }
        if patternid == UIA_SelectionItemPatternId
            && (node.selected.is_some() || node.actions.select)
        {
            return Ok(ISelectionItemProvider::from(self.this.clone()).into());
        }
        if patternid == UIA_SelectionPatternId && node.selection.is_some() {
            return Ok(ISelectionProvider::from(self.this.clone()).into());
        }
        if patternid == UIA_ValuePatternId && (node.value.is_some() || node.actions.set_value) {
            return Ok(IValueProvider::from(self.this.clone()).into());
        }
        Err(bridge_error(
            UIA_E_NOTSUPPORTED,
            "UIA pattern is not supported",
        ))
    }

    fn GetPropertyValue(
        &self,
        propertyid: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
    ) -> WinResult<VARIANT> {
        let node = self.node()?.node;
        let value = match propertyid {
            UIA_RuntimeIdPropertyId => {
                return runtime_id_variant(&self.state, &self.node_id, self.instance_key);
            }
            UIA_NamePropertyId => variant_string(&node.name),
            UIA_ControlTypePropertyId => variant_i32(role_control_type(node.role)),
            UIA_LocalizedControlTypePropertyId => variant_string(node.role.as_str()),
            UIA_BoundingRectanglePropertyId => return bounds_variant(node.bounds),
            UIA_AutomationIdPropertyId => variant_string(&node.id),
            UIA_ClassNamePropertyId => variant_string("MakepadAccessibility"),
            UIA_FrameworkIdPropertyId => variant_string("Makepad"),
            UIA_ProviderDescriptionPropertyId => variant_string("Rust Makepad Toolkit UIA bridge"),
            UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => variant_bool(true),
            UIA_IsEnabledPropertyId => variant_bool(node.enabled),
            UIA_IsOffscreenPropertyId => variant_bool(!node.visible),
            UIA_HasKeyboardFocusPropertyId => variant_bool(node.focused),
            UIA_IsKeyboardFocusablePropertyId => variant_bool(node.actions.focus || node.focused),
            UIA_ValueValuePropertyId => node
                .value
                .as_deref()
                .map(variant_string)
                .unwrap_or_default(),
            UIA_SelectionItemIsSelectedPropertyId => {
                return node
                    .selected
                    .map(variant_bool)
                    .ok_or_else(|| bridge_error(UIA_E_NOTSUPPORTED, "node is not selectable"));
            }
            UIA_SelectionCanSelectMultiplePropertyId => node
                .selection
                .map(|selection| variant_bool(selection.can_select_multiple))
                .ok_or_else(|| {
                    bridge_error(UIA_E_NOTSUPPORTED, "node is not a selection container")
                })?,
            UIA_SelectionIsSelectionRequiredPropertyId => node
                .selection
                .map(|selection| variant_bool(selection.is_selection_required))
                .ok_or_else(|| {
                    bridge_error(UIA_E_NOTSUPPORTED, "node is not a selection container")
                })?,
            _ => VARIANT::default(),
        };
        Ok(value)
    }

    fn HostRawElementProvider(&self) -> WinResult<IRawElementProviderSimple> {
        if self.is_root() {
            unsafe { UiaHostProviderFromHwnd(hwnd_from_value(self.state.hwnd_value)) }
        } else {
            Err(bridge_error(
                UIA_E_NOTSUPPORTED,
                "only the fragment root has a host provider",
            ))
        }
    }
}

impl IRawElementProviderFragment_Impl for Provider_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> WinResult<IRawElementProviderFragment> {
        match self.navigation_target(direction)? {
            Some(id) => Ok(self.fragment_for(id)),
            None => Err(bridge_error(
                UIA_E_ELEMENTNOTAVAILABLE,
                "navigation target does not exist",
            )),
        }
    }

    fn GetRuntimeId(&self) -> WinResult<*mut SAFEARRAY> {
        let _ = self.node()?;
        runtime_id(&self.state, &self.node_id, self.instance_key)
    }

    fn BoundingRectangle(&self) -> WinResult<UiaRect> {
        let node = self.node()?.node;
        Ok(UiaRect {
            left: node.bounds.x,
            top: node.bounds.y,
            width: node.bounds.width,
            height: node.bounds.height,
        })
    }

    fn GetEmbeddedFragmentRoots(&self) -> WinResult<*mut SAFEARRAY> {
        empty_unknown_array()
    }

    fn SetFocus(&self) -> WinResult<()> {
        self.action(AccessibilityAction::Focus)
    }

    fn FragmentRoot(&self) -> WinResult<IRawElementProviderFragmentRoot> {
        let root_id = self.current_snapshot().root_id.clone();
        Ok(Provider::new(self.state.clone(), root_id).into())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Provider_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> WinResult<IRawElementProviderFragment> {
        Ok(self.fragment_for(self.hit_test(x, y)?))
    }

    fn GetFocus(&self) -> WinResult<IRawElementProviderFragment> {
        Ok(self.fragment_for(self.focused_node()?))
    }
}

impl IInvokeProvider_Impl for Provider_Impl {
    fn Invoke(&self) -> WinResult<()> {
        self.action(AccessibilityAction::Invoke)
    }
}

impl IValueProvider_Impl for Provider_Impl {
    fn Value(&self) -> WinResult<BSTR> {
        Ok(BSTR::from(
            self.node()?.node.value.as_deref().unwrap_or_default(),
        ))
    }

    fn IsReadOnly(&self) -> WinResult<BOOL> {
        Ok(BOOL::from(!self.node()?.node.actions.set_value))
    }

    fn SetValue(&self, value: &PCWSTR) -> WinResult<()> {
        let value = unsafe { value.to_string().unwrap_or_default() };
        let value = if value == MAKEPAD_EMPTY_VALUE_SENTINEL {
            String::new()
        } else {
            value
        };
        self.action(AccessibilityAction::SetValue(value))
    }
}

impl ISelectionItemProvider_Impl for Provider_Impl {
    fn Select(&self) -> WinResult<()> {
        self.action(AccessibilityAction::Select)
    }

    fn AddToSelection(&self) -> WinResult<()> {
        self.action(AccessibilityAction::AddToSelection)
    }

    fn RemoveFromSelection(&self) -> WinResult<()> {
        self.action(AccessibilityAction::RemoveFromSelection)
    }

    fn IsSelected(&self) -> WinResult<BOOL> {
        self.node()?
            .node
            .selected
            .map(BOOL::from)
            .ok_or_else(|| bridge_error(UIA_E_NOTSUPPORTED, "node is not selectable"))
    }

    fn SelectionContainer(&self) -> WinResult<IRawElementProviderSimple> {
        Ok(self.simple_for(self.selection_container_id()?))
    }
}

impl ISelectionProvider_Impl for Provider_Impl {
    fn GetSelection(&self) -> WinResult<*mut SAFEARRAY> {
        let snapshot = self.current_snapshot();
        let node = snapshot.nodes.get(&self.node_id).ok_or_else(|| {
            bridge_error(UIA_E_ELEMENTNOTAVAILABLE, "accessibility node is stale")
        })?;
        if node.node.selection.is_none() {
            return Err(bridge_error(
                UIA_E_NOTSUPPORTED,
                "node is not a selection container",
            ));
        }
        let selected = node
            .children
            .iter()
            .filter_map(|child_id| snapshot.nodes.get(child_id))
            .filter(|child| child.node.selected == Some(true))
            .map(|child| child.node.id.clone())
            .collect::<Vec<_>>();
        drop(snapshot);
        provider_array(self, &selected)
    }

    fn CanSelectMultiple(&self) -> WinResult<BOOL> {
        self.node()?
            .node
            .selection
            .map(|selection| BOOL::from(selection.can_select_multiple))
            .ok_or_else(|| bridge_error(UIA_E_NOTSUPPORTED, "node is not a selection container"))
    }

    fn IsSelectionRequired(&self) -> WinResult<BOOL> {
        self.node()?
            .node
            .selection
            .map(|selection| BOOL::from(selection.is_selection_required))
            .ok_or_else(|| bridge_error(UIA_E_NOTSUPPORTED, "node is not a selection container"))
    }
}

fn provider_array(provider: &Provider, ids: &[String]) -> WinResult<*mut SAFEARRAY> {
    let array = unsafe { SafeArrayCreateVector(VT_UNKNOWN, 0, ids.len() as u32) };
    if array.is_null() {
        return Err(bridge_error(
            E_FAIL,
            "failed to allocate UIA provider SAFEARRAY",
        ));
    }
    for (index, id) in ids.iter().enumerate() {
        let simple = provider.simple_for(id.clone());
        let unknown: IUnknown = simple
            .cast()
            .map_err(|_| bridge_error(E_FAIL, "failed to cast UIA provider to IUnknown"))?;
        let raw = unknown.as_raw();
        let index = index as i32;
        if let Err(error) = unsafe { SafeArrayPutElement(array, &index, raw as *const c_void) } {
            unsafe {
                let _ = SafeArrayDestroy(array);
            }
            return Err(error);
        }
    }
    Ok(array)
}

fn empty_unknown_array() -> WinResult<*mut SAFEARRAY> {
    let array = unsafe { SafeArrayCreateVector(VT_UNKNOWN, 0, 0) };
    if array.is_null() {
        Err(bridge_error(
            E_FAIL,
            "failed to allocate empty UIA SAFEARRAY",
        ))
    } else {
        Ok(array)
    }
}

fn runtime_id_variant(
    state: &BridgeState,
    node_id: &str,
    instance_key: Option<u64>,
) -> WinResult<VARIANT> {
    Ok(variant_array(
        runtime_id(state, node_id, instance_key)?,
        VARENUM(VT_I4.0 | VT_ARRAY.0),
    ))
}

fn runtime_id(
    state: &BridgeState,
    node_id: &str,
    instance_key: Option<u64>,
) -> WinResult<*mut SAFEARRAY> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    state.hwnd_value.hash(&mut hasher);
    instance_key.hash(&mut hasher);
    node_id.hash(&mut hasher);
    let id = (hasher.finish() as i32) & 0x7fff_ffff;
    i32_array(&[UiaAppendRuntimeId as i32, id])
}

fn i32_array(values: &[i32]) -> WinResult<*mut SAFEARRAY> {
    let array = unsafe { SafeArrayCreateVector(VT_I4, 0, values.len() as u32) };
    if array.is_null() {
        return Err(bridge_error(
            E_FAIL,
            "failed to allocate UIA integer SAFEARRAY",
        ));
    }
    for (index, value) in values.iter().enumerate() {
        let index = index as i32;
        if let Err(error) =
            unsafe { SafeArrayPutElement(array, &index, value as *const i32 as *const c_void) }
        {
            unsafe {
                let _ = SafeArrayDestroy(array);
            }
            return Err(error);
        }
    }
    Ok(array)
}

fn bounds_variant(bounds: crate::AccessibilityRect) -> WinResult<VARIANT> {
    let values = [bounds.x, bounds.y, bounds.width, bounds.height];
    let array = unsafe { SafeArrayCreateVector(VT_R8, 0, values.len() as u32) };
    if array.is_null() {
        return Err(bridge_error(
            E_FAIL,
            "failed to allocate UIA bounds SAFEARRAY",
        ));
    }
    for (index, value) in values.iter().enumerate() {
        let index = index as i32;
        if let Err(error) =
            unsafe { SafeArrayPutElement(array, &index, value as *const f64 as *const c_void) }
        {
            unsafe {
                let _ = SafeArrayDestroy(array);
            }
            return Err(error);
        }
    }
    Ok(variant_array(array, VARENUM(VT_R8.0 | VT_ARRAY.0)))
}

fn variant_array(array: *mut SAFEARRAY, vt: VARENUM) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { parray: array },
            }),
        },
    }
}

fn variant_string(value: &str) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BSTR,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    bstrVal: ManuallyDrop::new(BSTR::from(value)),
                },
            }),
        },
    }
}

fn variant_i32(value: i32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: value },
            }),
        },
    }
}

fn variant_bool(value: bool) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    boolVal: if value { VARIANT_TRUE } else { VARIANT_BOOL(0) },
                },
            }),
        },
    }
}

fn role_control_type(role: AccessibilityRole) -> i32 {
    match role {
        AccessibilityRole::Button => 50_000,
        AccessibilityRole::CheckBox => 50_002,
        AccessibilityRole::ComboBox => 50_003,
        AccessibilityRole::Edit => 50_004,
        AccessibilityRole::Hyperlink => 50_005,
        AccessibilityRole::Image => 50_006,
        AccessibilityRole::ListItem => 50_007,
        AccessibilityRole::List => 50_008,
        AccessibilityRole::Menu => 50_009,
        AccessibilityRole::MenuBar => 50_010,
        AccessibilityRole::MenuItem => 50_011,
        AccessibilityRole::ProgressBar => 50_012,
        AccessibilityRole::RadioButton => 50_013,
        AccessibilityRole::ScrollBar => 50_014,
        AccessibilityRole::Slider => 50_015,
        AccessibilityRole::Spinner => 50_016,
        AccessibilityRole::StatusBar => 50_017,
        AccessibilityRole::Tab => 50_018,
        AccessibilityRole::TabItem => 50_019,
        AccessibilityRole::Text => 50_020,
        AccessibilityRole::ToolBar => 50_021,
        AccessibilityRole::Tree => 50_023,
        AccessibilityRole::TreeItem => 50_024,
        AccessibilityRole::Custom | AccessibilityRole::Unknown => 50_025,
        AccessibilityRole::Group => 50_026,
        AccessibilityRole::DataGrid => 50_028,
        AccessibilityRole::DataItem => 50_029,
        AccessibilityRole::Document => 50_030,
        AccessibilityRole::SplitButton => 50_031,
        AccessibilityRole::Window => 50_032,
        AccessibilityRole::Pane => 50_033,
        AccessibilityRole::TitleBar => 50_037,
    }
}

fn hwnd_from_value(value: isize) -> HWND {
    HWND(value as *mut c_void)
}

fn bridge_error(code: HRESULT, message: impl Into<String>) -> Error {
    let message = message.into();
    Error::new(code, &message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accessibility::{AccessibilityActions, AccessibilityNode, AccessibilityRect};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn indexed_button_snapshot(generation: u64, instance_key: u64) -> AccessibilityIndex {
        let mut root = AccessibilityNode::new(
            "app.window",
            AccessibilityRole::Window,
            "Test window",
            AccessibilityRect::default(),
        );
        let mut accept = AccessibilityNode::new(
            "dialog.confirmation.accept",
            AccessibilityRole::Button,
            "Подтвердить",
            AccessibilityRect::default(),
        );
        accept.actions = AccessibilityActions {
            invoke: true,
            ..Default::default()
        };
        accept.instance_key = Some(instance_key);
        root.children.push(accept);
        AccessibilityTree::new(root)
            .expect("valid test accessibility tree")
            .with_generation(generation)
            .index()
            .expect("index test accessibility tree")
    }

    #[test]
    fn stale_provider_cannot_invoke_replacement_with_same_automation_id() {
        let invoked = Arc::new(AtomicUsize::new(0));
        let recorded = invoked.clone();
        let handler: AccessibilityActionHandler = Arc::new(move |_| {
            recorded.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let state = Arc::new(BridgeState {
            hwnd_value: 0,
            old_wnd_proc: 0,
            snapshot: RwLock::new(indexed_button_snapshot(1, 1)),
            action_handler: handler,
        });
        let old = Provider::new(state.clone(), "dialog.confirmation.accept".to_owned());

        *state.snapshot.write().expect("test snapshot lock poisoned") =
            indexed_button_snapshot(2, 2);

        let error = old
            .action(AccessibilityAction::Invoke)
            .expect_err("old provider must be stale after snapshot replacement");
        assert_eq!(error.code(), UIA_E_ELEMENTNOTAVAILABLE);
        assert_eq!(invoked.load(Ordering::SeqCst), 0);

        Provider::new(state, "dialog.confirmation.accept".to_owned())
            .action(AccessibilityAction::Invoke)
            .expect("current provider may invoke the current control");
        assert_eq!(invoked.load(Ordering::SeqCst), 1);
    }
}
