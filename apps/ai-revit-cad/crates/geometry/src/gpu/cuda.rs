//! Minimal Windows CUDA Driver ABI. No CUDA SDK, runtime, or managed allocation.
//! Every context is private to this module; calls are serialized by its owner.
use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
type Context = *mut c_void;
type Module = *mut c_void;
type Function = *mut c_void;
type Stream = *mut c_void;
type DevicePtr = u64;
type ResultCode = c_int;
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> c_int;
}
struct Library(*mut c_void);
// The module is kept loaded until all copied immutable function pointers die.
unsafe impl Send for Library {}
unsafe impl Sync for Library {}
impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}
struct Api {
    _library: Library,
    init: unsafe extern "system" fn(c_uint) -> ResultCode,
    version: unsafe extern "system" fn(*mut c_int) -> ResultCode,
    device_count: unsafe extern "system" fn(*mut c_int) -> ResultCode,
    device_get: unsafe extern "system" fn(*mut c_int, c_int) -> ResultCode,
    device_name: unsafe extern "system" fn(*mut c_char, c_int, c_int) -> ResultCode,
    device_luid: unsafe extern "system" fn(*mut c_char, *mut c_uint, c_int) -> ResultCode,
    device_memory: unsafe extern "system" fn(*mut usize, c_int) -> ResultCode,
    device_attribute: unsafe extern "system" fn(*mut c_int, c_int, c_int) -> ResultCode,
    error_name: unsafe extern "system" fn(ResultCode, *mut *const c_char) -> ResultCode,
    context_create: unsafe extern "system" fn(*mut Context, c_uint, c_int) -> ResultCode,
    context_destroy: unsafe extern "system" fn(Context) -> ResultCode,
    context_push: unsafe extern "system" fn(Context) -> ResultCode,
    context_pop: unsafe extern "system" fn(*mut Context) -> ResultCode,
    synchronize: unsafe extern "system" fn() -> ResultCode,
    module_load: unsafe extern "system" fn(*mut Module, *const c_void) -> ResultCode,
    module_unload: unsafe extern "system" fn(Module) -> ResultCode,
    module_function: unsafe extern "system" fn(*mut Function, Module, *const c_char) -> ResultCode,
    memory_info: unsafe extern "system" fn(*mut usize, *mut usize) -> ResultCode,
    memory_alloc: unsafe extern "system" fn(*mut DevicePtr, usize) -> ResultCode,
    memory_free: unsafe extern "system" fn(DevicePtr) -> ResultCode,
    copy_to_device: unsafe extern "system" fn(DevicePtr, *const c_void, usize) -> ResultCode,
    copy_to_host: unsafe extern "system" fn(*mut c_void, DevicePtr, usize) -> ResultCode,
    launch: unsafe extern "system" fn(
        Function,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        c_uint,
        Stream,
        *mut *mut c_void,
        *mut *mut c_void,
    ) -> ResultCode,
}
impl Api {
    fn load() -> Result<Arc<Self>, String> {
        // Fixed system DLL, never load an application-directory lookalike.
        let name: Vec<u16> = "nvcuda.dll".encode_utf16().chain([0]).collect();
        let library =
            Library(unsafe { LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), 0x800) });
        if library.0.is_null() {
            return Err("cuda_driver_system_dll_unavailable".into());
        }
        macro_rules! symbol {
            ($name:literal,$ty:ty) => {{
                let pointer =
                    unsafe { GetProcAddress(library.0, concat!($name, "\0").as_ptr().cast()) };
                if pointer.is_null() {
                    return Err(concat!("cuda_missing_symbol:", $name).into());
                }
                // Each type below is the documented versioned CUDA Driver export ABI.
                unsafe { std::mem::transmute::<*mut c_void, $ty>(pointer) }
            }};
        }
        Ok(Arc::new(Self {
            init: symbol!("cuInit", unsafe extern "system" fn(c_uint) -> ResultCode),
            version: symbol!(
                "cuDriverGetVersion",
                unsafe extern "system" fn(*mut c_int) -> ResultCode
            ),
            device_count: symbol!(
                "cuDeviceGetCount",
                unsafe extern "system" fn(*mut c_int) -> ResultCode
            ),
            device_get: symbol!(
                "cuDeviceGet",
                unsafe extern "system" fn(*mut c_int, c_int) -> ResultCode
            ),
            device_name: symbol!(
                "cuDeviceGetName",
                unsafe extern "system" fn(*mut c_char, c_int, c_int) -> ResultCode
            ),
            device_luid: symbol!(
                "cuDeviceGetLuid",
                unsafe extern "system" fn(*mut c_char, *mut c_uint, c_int) -> ResultCode
            ),
            device_memory: symbol!(
                "cuDeviceTotalMem_v2",
                unsafe extern "system" fn(*mut usize, c_int) -> ResultCode
            ),
            device_attribute: symbol!(
                "cuDeviceGetAttribute",
                unsafe extern "system" fn(*mut c_int, c_int, c_int) -> ResultCode
            ),
            error_name: symbol!(
                "cuGetErrorName",
                unsafe extern "system" fn(ResultCode, *mut *const c_char) -> ResultCode
            ),
            context_create: symbol!(
                "cuCtxCreate_v2",
                unsafe extern "system" fn(*mut Context, c_uint, c_int) -> ResultCode
            ),
            context_destroy: symbol!(
                "cuCtxDestroy_v2",
                unsafe extern "system" fn(Context) -> ResultCode
            ),
            context_push: symbol!(
                "cuCtxPushCurrent_v2",
                unsafe extern "system" fn(Context) -> ResultCode
            ),
            context_pop: symbol!(
                "cuCtxPopCurrent_v2",
                unsafe extern "system" fn(*mut Context) -> ResultCode
            ),
            synchronize: symbol!(
                "cuCtxSynchronize",
                unsafe extern "system" fn() -> ResultCode
            ),
            module_load: symbol!(
                "cuModuleLoadData",
                unsafe extern "system" fn(*mut Module, *const c_void) -> ResultCode
            ),
            module_unload: symbol!(
                "cuModuleUnload",
                unsafe extern "system" fn(Module) -> ResultCode
            ),
            module_function: symbol!(
                "cuModuleGetFunction",
                unsafe extern "system" fn(*mut Function, Module, *const c_char) -> ResultCode
            ),
            memory_info: symbol!(
                "cuMemGetInfo_v2",
                unsafe extern "system" fn(*mut usize, *mut usize) -> ResultCode
            ),
            memory_alloc: symbol!(
                "cuMemAlloc_v2",
                unsafe extern "system" fn(*mut DevicePtr, usize) -> ResultCode
            ),
            memory_free: symbol!(
                "cuMemFree_v2",
                unsafe extern "system" fn(DevicePtr) -> ResultCode
            ),
            copy_to_device: symbol!(
                "cuMemcpyHtoD_v2",
                unsafe extern "system" fn(DevicePtr, *const c_void, usize) -> ResultCode
            ),
            copy_to_host: symbol!(
                "cuMemcpyDtoH_v2",
                unsafe extern "system" fn(*mut c_void, DevicePtr, usize) -> ResultCode
            ),
            launch: symbol!(
                "cuLaunchKernel",
                unsafe extern "system" fn(
                    Function,
                    c_uint,
                    c_uint,
                    c_uint,
                    c_uint,
                    c_uint,
                    c_uint,
                    c_uint,
                    Stream,
                    *mut *mut c_void,
                    *mut *mut c_void,
                ) -> ResultCode
            ),
            _library: library,
        }))
    }
    fn check(&self, code: ResultCode, operation: &str) -> Result<(), String> {
        if code == 0 {
            return Ok(());
        }
        let mut name = std::ptr::null();
        let resolved = unsafe { (self.error_name)(code, &mut name) };
        let name = if resolved == 0 && !name.is_null() {
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned()
        } else {
            "unknown_cuda_error".into()
        };
        Err(format!("{operation}:{name}({code})"))
    }
}
#[derive(Default)]
struct AllocationLedger {
    owned: AtomicU64,
    peak: AtomicU64,
    free_errors: AtomicU64,
    pop_errors: AtomicU64,
    push_errors: AtomicU64,
    destroy_errors: AtomicU64,
    unload_errors: AtomicU64,
    synchronize_errors: AtomicU64,
    unusable: AtomicBool,
    shutdown_attempted: AtomicBool,
    context_destroyed: AtomicBool,
    last_error: Mutex<Option<String>>,
}
impl AllocationLedger {
    fn record_error(&self, counter: &AtomicU64, error: String) {
        counter.fetch_add(1, Ordering::Relaxed);
        self.unusable.store(true, Ordering::Release);
        *self
            .last_error
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(error);
    }
    fn failure_count(&self) -> u64 {
        [
            &self.free_errors,
            &self.pop_errors,
            &self.push_errors,
            &self.destroy_errors,
            &self.unload_errors,
            &self.synchronize_errors,
        ]
        .into_iter()
        .map(|counter| counter.load(Ordering::Relaxed))
        .fold(0u64, u64::saturating_add)
    }
    fn diagnostic(&self) -> Option<String> {
        if self.failure_count() == 0 {
            return None;
        }
        let last = self
            .last_error
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        Some(format!(
            "cuda_cleanup_failed:free={},pop={},push={},destroy={},unload={},sync={};last={}",
            self.free_errors.load(Ordering::Relaxed),
            self.pop_errors.load(Ordering::Relaxed),
            self.push_errors.load(Ordering::Relaxed),
            self.destroy_errors.load(Ordering::Relaxed),
            self.unload_errors.load(Ordering::Relaxed),
            self.synchronize_errors.load(Ordering::Relaxed),
            last.as_deref().unwrap_or("unknown_cleanup_error")
        ))
    }
    fn observation(&self) -> Value {
        let failures = self.failure_count();
        let destroyed = self.context_destroyed.load(Ordering::Acquire);
        let attempted = self.shutdown_attempted.load(Ordering::Acquire);
        let last = self
            .last_error
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        json!({
            "status":if destroyed {
                if failures==0 {"shutdown_verified"} else {"shutdown_with_errors"}
            } else if attempted {"shutdown_unverified"}
                else if self.unusable.load(Ordering::Acquire) {"unusable"} else {"active"},
            "context_destroyed":destroyed,
            "failure_count":failures,
            "free_errors":self.free_errors.load(Ordering::Relaxed),
            "context_pop_errors":self.pop_errors.load(Ordering::Relaxed),
            "context_push_errors":self.push_errors.load(Ordering::Relaxed),
            "context_destroy_errors":self.destroy_errors.load(Ordering::Relaxed),
            "module_unload_errors":self.unload_errors.load(Ordering::Relaxed),
            "synchronize_errors":self.synchronize_errors.load(Ordering::Relaxed),
            "last_error":last
        })
    }
}
fn append_cleanup_error(error: String, cleanup: Result<(), String>) -> String {
    match cleanup {
        Ok(()) => error,
        Err(cleanup) => format!("{error};{cleanup}"),
    }
}
struct CurrentContext {
    api: Arc<Api>,
    ledger: Arc<AllocationLedger>,
    expected: Context,
    active: bool,
    operation: &'static str,
}
impl CurrentContext {
    fn detach(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        // A failed pop leaves stack state unknown. Do not blindly pop again in
        // Drop: it could remove a caller's context if the first call took effect.
        self.active = false;
        let mut popped = std::ptr::null_mut();
        let result = self
            .api
            .check(
                unsafe { (self.api.context_pop)(&mut popped) },
                self.operation,
            )
            .and_then(|()| {
                if popped == self.expected {
                    Ok(())
                } else {
                    Err(format!("{}:unexpected_context_handle", self.operation))
                }
            });
        if let Err(error) = &result {
            self.ledger
                .record_error(&self.ledger.pop_errors, error.clone());
        }
        result
    }
    fn finish(mut self) -> Result<(), String> {
        self.detach()
    }
}
impl Drop for CurrentContext {
    fn drop(&mut self) {
        let _ = self.detach();
    }
}
struct DeviceBuffer<'a> {
    api: &'a Api,
    ledger: &'a AllocationLedger,
    pointer: DevicePtr,
    bytes: u64,
}
impl<'a> DeviceBuffer<'a> {
    fn new(api: &'a Api, ledger: &'a AllocationLedger, bytes: usize) -> Result<Self, String> {
        if bytes == 0 {
            return Err("zero_sized_cuda_allocation".into());
        }
        let mut pointer = 0;
        api.check(
            unsafe { (api.memory_alloc)(&mut pointer, bytes) },
            "cuMemAlloc_v2",
        )?;
        let owned = ledger.owned.fetch_add(bytes as u64, Ordering::Relaxed) + bytes as u64;
        ledger.peak.fetch_max(owned, Ordering::Relaxed);
        Ok(Self {
            api,
            ledger,
            pointer,
            bytes: bytes as u64,
        })
    }
}
impl Drop for DeviceBuffer<'_> {
    fn drop(&mut self) {
        let result = self.api.check(
            unsafe { (self.api.memory_free)(self.pointer) },
            "cuMemFree_v2",
        );
        match result {
            Ok(()) => {
                self.ledger.owned.fetch_sub(self.bytes, Ordering::Relaxed);
            }
            Err(error) => self.ledger.record_error(&self.ledger.free_errors, error),
        }
    }
}
pub(super) struct Runtime {
    api: Arc<Api>,
    context: Context,
    module: Module,
    mesh_kernel: Function,
    bounds_kernel: Function,
    luid: [u8; 8],
    name: String,
    driver_version: c_int,
    total_bytes: usize,
    capability: [c_int; 2],
    allocations: Arc<AllocationLedger>,
}
// A context may migrate between threads through Push/Pop; the parent mutex
// prevents concurrent calls and no device pointer escapes this module.
unsafe impl Send for Runtime {}
impl Runtime {
    pub(super) fn new(expected_luid: [u8; 8]) -> Result<Self, String> {
        let api = Api::load()?;
        api.check(unsafe { (api.init)(0) }, "cuInit")?;
        let mut count = 0;
        api.check(
            unsafe { (api.device_count)(&mut count) },
            "cuDeviceGetCount",
        )?;
        let mut selected = None;
        for ordinal in 0..count {
            let (mut device, mut node_mask) = (0, 0);
            let mut luid = [0u8; 8];
            api.check(
                unsafe { (api.device_get)(&mut device, ordinal) },
                "cuDeviceGet",
            )?;
            if unsafe { (api.device_luid)(luid.as_mut_ptr().cast(), &mut node_mask, device) } == 0
                && luid == expected_luid
                && node_mask == 1
            {
                selected = Some(device);
                break;
            }
        }
        let device = selected.ok_or("cuda_device_matching_d3d11_luid_not_found_or_nonzero_node")?;
        let mut name = [0 as c_char; 256];
        api.check(
            unsafe { (api.device_name)(name.as_mut_ptr(), name.len() as c_int, device) },
            "cuDeviceGetName",
        )?;
        let name = unsafe { CStr::from_ptr(name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let (mut driver_version, mut total_bytes) = (0, 0);
        api.check(
            unsafe { (api.version)(&mut driver_version) },
            "cuDriverGetVersion",
        )?;
        api.check(
            unsafe { (api.device_memory)(&mut total_bytes, device) },
            "cuDeviceTotalMem_v2",
        )?;
        let mut capability = [0, 0];
        api.check(
            unsafe { (api.device_attribute)(&mut capability[0], 75, device) },
            "cuDeviceGetAttribute_major",
        )?;
        api.check(
            unsafe { (api.device_attribute)(&mut capability[1], 76, device) },
            "cuDeviceGetAttribute_minor",
        )?;
        let mut context = std::ptr::null_mut();
        // Blocking synchronization avoids a CPU busy-wait. The legacy v2 symbol
        // deliberately uses the three-argument ABI, not cuCtxCreate v4's ABI.
        api.check(
            unsafe { (api.context_create)(&mut context, 4, device) },
            "cuCtxCreate_v2",
        )?;
        if context.is_null() {
            return Err("cuCtxCreate_v2:null_context".into());
        }
        let mut runtime = Self {
            api,
            context,
            module: std::ptr::null_mut(),
            mesh_kernel: std::ptr::null_mut(),
            bounds_kernel: std::ptr::null_mut(),
            luid: expected_luid,
            name,
            driver_version,
            total_bytes,
            capability,
            allocations: Arc::new(AllocationLedger::default()),
        };
        // cuCtxCreate made this context current already; verify the initial pop
        // without issuing another push. Runtime owns cleanup on every exit.
        let initial = CurrentContext {
            api: runtime.api.clone(),
            ledger: runtime.allocations.clone(),
            expected: context,
            active: true,
            operation: "cuCtxPopCurrent_v2_init",
        };
        if let Err(error) = initial.finish() {
            return Err(append_cleanup_error(error, runtime.shutdown()));
        }
        let current = match runtime.current() {
            Ok(current) => current,
            Err(error) => return Err(append_cleanup_error(error, runtime.shutdown())),
        };
        let initialization = (|| -> Result<(), String> {
            let ptx = CString::new(include_str!("kernels.ptx")).map_err(|_| "ptx_contains_nul")?;
            runtime.api.check(
                unsafe { (runtime.api.module_load)(&mut runtime.module, ptx.as_ptr().cast()) },
                "cuModuleLoadData",
            )?;
            runtime.api.check(
                unsafe {
                    (runtime.api.module_function)(
                        &mut runtime.mesh_kernel,
                        runtime.module,
                        c"mesh_contributions".as_ptr(),
                    )
                },
                "cuModuleGetFunction_mesh",
            )?;
            runtime.api.check(
                unsafe {
                    (runtime.api.module_function)(
                        &mut runtime.bounds_kernel,
                        runtime.module,
                        c"aabb_overlaps".as_ptr(),
                    )
                },
                "cuModuleGetFunction_bounds",
            )?;
            Ok(())
        })();
        let detached = current.finish();
        let initialized = match initialization {
            Ok(()) => detached,
            Err(error) => Err(append_cleanup_error(error, detached)),
        };
        if let Err(error) = initialized {
            return Err(append_cleanup_error(error, runtime.shutdown()));
        }
        Ok(runtime)
    }
    fn current(&self) -> Result<CurrentContext, String> {
        if self.allocations.unusable.load(Ordering::Acquire) {
            return Err(self
                .allocations
                .diagnostic()
                .unwrap_or_else(|| "cuda_runtime_unusable".into()));
        }
        self.push_current()
    }
    fn push_current(&self) -> Result<CurrentContext, String> {
        if self.context.is_null() {
            return Err("cuda_runtime_shutdown".into());
        }
        if let Err(error) = self.api.check(
            unsafe { (self.api.context_push)(self.context) },
            "cuCtxPushCurrent_v2",
        ) {
            self.allocations
                .record_error(&self.allocations.push_errors, error.clone());
            return Err(error);
        }
        Ok(CurrentContext {
            api: self.api.clone(),
            ledger: self.allocations.clone(),
            expected: self.context,
            active: true,
            operation: "cuCtxPopCurrent_v2",
        })
    }
    pub(super) fn identity(&self) -> Value {
        json!({"name":self.name,"luid_bytes":self.luid,"matches_d3d11_luid":true,
            "driver_api_version":self.driver_version,"total_device_bytes":self.total_bytes,
            "compute_capability":self.capability,"device_node_mask":1,
            "context":if self.context.is_null() {"destroyed"} else {"private"},
            "usable":!self.context.is_null() && !self.allocations.unusable.load(Ordering::Acquire),
            "owned_device_bytes":self.allocations.owned.load(Ordering::Relaxed),
            "peak_device_bytes":self.allocations.peak.load(Ordering::Relaxed),
            "cleanup_failure_count":self.allocations.failure_count(),
            "cleanup":self.allocations.observation()})
    }
    fn memory_admission(&self, bytes: usize) -> Result<(), String> {
        let (mut free, mut total) = (0, 0);
        self.api.check(
            unsafe { (self.api.memory_info)(&mut free, &mut total) },
            "cuMemGetInfo_v2",
        )?;
        if bytes > 256 * 1024 * 1024 || bytes > free.saturating_sub(256 * 1024 * 1024) {
            return Err("cuda_device_free_headroom_or_256mib_limit".into());
        }
        Ok(())
    }
    fn launch(
        &self,
        kernel: Function,
        count: usize,
        parameters: &mut [*mut c_void],
    ) -> Result<(), String> {
        let count = u32::try_from(count).map_err(|_| "cuda_item_count_overflow")?;
        self.api.check(
            unsafe {
                (self.api.launch)(
                    kernel,
                    count.div_ceil(128),
                    1,
                    1,
                    128,
                    1,
                    1,
                    0,
                    std::ptr::null_mut(),
                    parameters.as_mut_ptr(),
                    std::ptr::null_mut(),
                )
            },
            "cuLaunchKernel",
        )?;
        self.api
            .check(unsafe { (self.api.synchronize)() }, "cuCtxSynchronize")
    }
    pub(super) fn mesh_contributions(
        &mut self,
        triangles: &[[f64; 15]],
    ) -> Result<Vec<[f64; 2]>, String> {
        if triangles.is_empty() {
            return Ok(Vec::new());
        }
        let input_bytes = triangles
            .len()
            .checked_mul(120)
            .ok_or("mesh_input_size_overflow")?;
        let output_bytes = triangles
            .len()
            .checked_mul(16)
            .ok_or("mesh_output_size_overflow")?;
        let current = self.current()?;
        self.memory_admission(
            input_bytes
                .checked_add(output_bytes)
                .ok_or("mesh_size_overflow")?,
        )?;
        let input = DeviceBuffer::new(&self.api, &self.allocations, input_bytes)?;
        let output = DeviceBuffer::new(&self.api, &self.allocations, output_bytes)?;
        self.api.check(
            unsafe {
                (self.api.copy_to_device)(input.pointer, triangles.as_ptr().cast(), input_bytes)
            },
            "cuMemcpyHtoD_mesh",
        )?;
        let (mut input_pointer, mut output_pointer, mut count) = (
            input.pointer,
            output.pointer,
            u32::try_from(triangles.len()).map_err(|_| "mesh_count_overflow")?,
        );
        let mut parameters = [
            (&mut input_pointer as *mut u64).cast(),
            (&mut output_pointer as *mut u64).cast(),
            (&mut count as *mut u32).cast(),
        ];
        self.launch(self.mesh_kernel, triangles.len(), &mut parameters)?;
        let mut values = vec![[0.0; 2]; triangles.len()];
        self.api.check(
            unsafe {
                (self.api.copy_to_host)(values.as_mut_ptr().cast(), output.pointer, output_bytes)
            },
            "cuMemcpyDtoH_mesh",
        )?;
        drop(output);
        drop(input);
        if self.allocations.owned.load(Ordering::Relaxed) != 0 {
            return Err("cuda_device_cleanup_failed".into());
        }
        current.finish()?;
        Ok(values)
    }
    pub(super) fn aabb_overlaps(
        &mut self,
        bounds: &[[f64; 6]],
        query: [f64; 6],
    ) -> Result<Vec<bool>, String> {
        if bounds.is_empty() {
            return Ok(Vec::new());
        }
        let input_bytes = bounds
            .len()
            .checked_mul(48)
            .ok_or("bounds_input_size_overflow")?;
        let output_bytes = bounds
            .len()
            .checked_mul(4)
            .ok_or("bounds_output_size_overflow")?;
        let current = self.current()?;
        self.memory_admission(
            input_bytes
                .checked_add(output_bytes)
                .and_then(|v| v.checked_add(48))
                .ok_or("bounds_size_overflow")?,
        )?;
        let input = DeviceBuffer::new(&self.api, &self.allocations, input_bytes)?;
        let query_buffer = DeviceBuffer::new(&self.api, &self.allocations, 48)?;
        let output = DeviceBuffer::new(&self.api, &self.allocations, output_bytes)?;
        self.api.check(
            unsafe {
                (self.api.copy_to_device)(input.pointer, bounds.as_ptr().cast(), input_bytes)
            },
            "cuMemcpyHtoD_bounds",
        )?;
        self.api.check(
            unsafe { (self.api.copy_to_device)(query_buffer.pointer, query.as_ptr().cast(), 48) },
            "cuMemcpyHtoD_query",
        )?;
        let (mut input_pointer, mut query_pointer, mut output_pointer, mut count) = (
            input.pointer,
            query_buffer.pointer,
            output.pointer,
            u32::try_from(bounds.len()).map_err(|_| "bounds_count_overflow")?,
        );
        let mut parameters = [
            (&mut input_pointer as *mut u64).cast(),
            (&mut query_pointer as *mut u64).cast(),
            (&mut output_pointer as *mut u64).cast(),
            (&mut count as *mut u32).cast(),
        ];
        self.launch(self.bounds_kernel, bounds.len(), &mut parameters)?;
        let mut values = vec![0u32; bounds.len()];
        self.api.check(
            unsafe {
                (self.api.copy_to_host)(values.as_mut_ptr().cast(), output.pointer, output_bytes)
            },
            "cuMemcpyDtoH_bounds",
        )?;
        if values.iter().any(|v| *v > 1) {
            return Err("cuda_invalid_aabb_boolean".into());
        }
        drop(output);
        drop(query_buffer);
        drop(input);
        if self.allocations.owned.load(Ordering::Relaxed) != 0 {
            return Err("cuda_device_cleanup_failed".into());
        }
        current.finish()?;
        Ok(values.into_iter().map(|v| v != 0).collect())
    }
    /// Explicit cleanup keeps its result available to the consumer's diagnostic.
    /// Drop retries destruction once if an explicit attempt could not destroy it.
    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        self.allocations
            .shutdown_attempted
            .store(true, Ordering::Release);
        if !self.context.is_null() {
            // A failed pop leaves the caller's context stack unknown. Avoid
            // pushing again in that case; destroy this private context directly.
            if self.allocations.pop_errors.load(Ordering::Relaxed) == 0 {
                if let Ok(current) = self.push_current() {
                    if let Err(error) = self.api.check(
                        unsafe { (self.api.synchronize)() },
                        "cuCtxSynchronize_shutdown",
                    ) {
                        self.allocations
                            .record_error(&self.allocations.synchronize_errors, error);
                    }
                    if !self.module.is_null() {
                        match self.api.check(
                            unsafe { (self.api.module_unload)(self.module) },
                            "cuModuleUnload",
                        ) {
                            Ok(()) => {
                                self.module = std::ptr::null_mut();
                                self.mesh_kernel = std::ptr::null_mut();
                                self.bounds_kernel = std::ptr::null_mut();
                            }
                            Err(error) => self
                                .allocations
                                .record_error(&self.allocations.unload_errors, error),
                        }
                    }
                    let _ = current.finish(); // finish records a failed pop.
                }
            }
            match self.api.check(
                unsafe { (self.api.context_destroy)(self.context) },
                "cuCtxDestroy_v2",
            ) {
                Ok(()) => {
                    self.context = std::ptr::null_mut();
                    self.module = std::ptr::null_mut();
                    self.mesh_kernel = std::ptr::null_mut();
                    self.bounds_kernel = std::ptr::null_mut();
                    // Successful context destruction releases any allocation
                    // whose individual cuMemFree returned an error. Retain all
                    // historical failure counters instead of reporting clean.
                    self.allocations.owned.store(0, Ordering::Relaxed);
                    self.allocations
                        .context_destroyed
                        .store(true, Ordering::Release);
                }
                Err(error) => self
                    .allocations
                    .record_error(&self.allocations.destroy_errors, error),
            }
        }
        match self.allocations.diagnostic() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
