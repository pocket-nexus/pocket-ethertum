//! A QuickJS runtime and context of the mods' own, with a time budget per call.
//!
//! `libquickjs-sys` declares part of QuickJS's API; the rest of what the realm
//! needs (interrupt handler, memory limit and usage, strings, opaque pointer,
//! stack top) is declared here against the same static library.
//!
//! Every call into the realm goes through [`Realm::eval`] or [`Realm::call`]:
//! they move the stack top to the calling thread's stack, arm the budget and
//! turn an exception into an [`Exception`] with the script's message and stack.
//! QuickJS polls the interrupt handler every 10 000 bytecode operations; past
//! the deadline the handler stops the script with an uncatchable error, which
//! no `try` in a mod can swallow.

use core::ffi::{c_char, c_int, c_void};
use std::ffi::CString;
use std::time::{Duration, Instant};

use libquickjs_sys as q;
pub use q::{JSContext, JSRuntime, JSValue};

#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct MemoryUsage {
    pub malloc_size: i64,
    pub malloc_limit: i64,
    pub memory_used_size: i64,
    pub malloc_count: i64,
    pub memory_used_count: i64,
    pub atom_count: i64,
    pub atom_size: i64,
    pub str_count: i64,
    pub str_size: i64,
    pub obj_count: i64,
    pub obj_size: i64,
    pub prop_count: i64,
    pub prop_size: i64,
    pub shape_count: i64,
    pub shape_size: i64,
    pub js_func_count: i64,
    pub js_func_size: i64,
    pub js_func_code_size: i64,
    pub js_func_pc2line_count: i64,
    pub js_func_pc2line_size: i64,
    pub c_func_count: i64,
    pub array_count: i64,
    pub fast_array_count: i64,
    pub fast_array_elements: i64,
    pub binary_object_count: i64,
    pub binary_object_size: i64,
}

type InterruptHandler = unsafe extern "C" fn(rt: *mut JSRuntime, opaque: *mut c_void) -> c_int;

extern "C" {
    fn JS_SetInterruptHandler(rt: *mut JSRuntime, cb: Option<InterruptHandler>, opaque: *mut c_void);
    fn JS_NewStringLen(ctx: *mut JSContext, s: *const c_char, len: usize) -> JSValue;
    fn JS_SetMemoryLimit(rt: *mut JSRuntime, limit: usize);
    fn JS_SetMaxStackSize(rt: *mut JSRuntime, size: usize);
    fn JS_UpdateStackTop(rt: *mut JSRuntime);
    fn JS_ComputeMemoryUsage(rt: *mut JSRuntime, s: *mut MemoryUsage);
    fn JS_SetContextOpaque(ctx: *mut JSContext, opaque: *mut c_void);
    fn JS_GetContextOpaque(ctx: *mut JSContext) -> *mut c_void;
}

const JS_EVAL_FLAG_STRICT: c_int = 1 << 3;
const TAG_NULL: i64 = 2;

pub const NULL: JSValue = JSValue { u: q::JSValueUnion { uint64: 0 }, tag: TAG_NULL };

/// A script's exception: its message and stack as QuickJS formats them, and whether the budget stopped it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exception {
    pub message: String,
    pub stack: String,
    pub interrupted: bool,
}

struct Budget {
    deadline: Option<Instant>,
    tripped: bool,
}

unsafe extern "C" fn interrupt(_rt: *mut JSRuntime, opaque: *mut c_void) -> c_int {
    let b = &mut *(opaque as *mut Budget);
    match b.deadline {
        Some(d) if Instant::now() >= d => {
            b.tripped = true;
            1
        }
        _ => 0,
    }
}

pub struct Realm {
    pub rt: *mut JSRuntime,
    pub ctx: *mut JSContext,
    budget: Box<Budget>,
}

impl Realm {
    /// A runtime of at most `memory_limit` bytes and `stack` bytes of native stack per call.
    pub fn new(memory_limit: usize, stack: usize) -> Realm {
        unsafe {
            let rt = q::JS_NewRuntime();
            assert!(!rt.is_null(), "QuickJS could not allocate a runtime");
            JS_SetMemoryLimit(rt, memory_limit);
            JS_SetMaxStackSize(rt, stack);
            let ctx = q::JS_NewContext(rt);
            assert!(!ctx.is_null(), "QuickJS could not allocate a context");
            let mut budget = Box::new(Budget { deadline: None, tripped: false });
            JS_SetInterruptHandler(rt, Some(interrupt), (&mut *budget as *mut Budget).cast());
            Realm { rt, ctx, budget }
        }
    }

    /// # Safety
    /// `p` stays valid until it is replaced; native functions of the realm read it during calls.
    pub unsafe fn set_opaque(&self, p: *mut c_void) {
        JS_SetContextOpaque(self.ctx, p)
    }

    /// # Safety
    /// `ctx` is a context of a live realm.
    pub unsafe fn opaque(ctx: *mut JSContext) -> *mut c_void {
        JS_GetContextOpaque(ctx)
    }

    fn arm(&mut self, budget: Duration) {
        unsafe { JS_UpdateStackTop(self.rt) };
        self.budget.tripped = false;
        self.budget.deadline = Some(Instant::now() + budget);
    }

    fn disarm(&mut self) -> bool {
        self.budget.deadline = None;
        self.budget.tripped
    }

    /// Evaluates a script as `file`, in strict mode, within `budget`.
    pub fn eval(&mut self, file: &str, source: &str, budget: Duration) -> Result<JSValue, Exception> {
        let name = CString::new(file.replace('\0', "")).unwrap();
        // QuickJS reads one byte past the source: it must end with a NUL.
        let mut text = Vec::with_capacity(source.len() + 1);
        text.extend_from_slice(source.as_bytes());
        text.push(0);
        self.arm(budget);
        let v = unsafe { q::JS_Eval(self.ctx, text.as_ptr().cast(), source.len(), name.as_ptr(), q::JS_EVAL_TYPE_GLOBAL as c_int | JS_EVAL_FLAG_STRICT) };
        let interrupted = self.disarm();
        self.check(v, interrupted)
    }

    /// Calls `f` with `args` (borrowed; the caller frees them) within `budget`.
    pub fn call(&mut self, f: JSValue, args: &mut [JSValue], budget: Duration) -> Result<JSValue, Exception> {
        self.arm(budget);
        let v = unsafe { q::JS_Call(self.ctx, f, q::JS_UNDEFINED, args.len() as c_int, args.as_mut_ptr()) };
        let interrupted = self.disarm();
        self.check(v, interrupted)
    }

    fn check(&mut self, v: JSValue, interrupted: bool) -> Result<JSValue, Exception> {
        unsafe {
            if !q::JS_IsException(v) {
                return Ok(v);
            }
            let e = q::JS_GetException(self.ctx);
            let message = self.string(e);
            let stack = if q::JS_IsObject(e) {
                let s = self.get(e, "stack");
                let text = if q::JS_IsString(s) { self.string(s) } else { String::new() };
                q::JS_FreeValue(self.ctx, s);
                text
            } else {
                String::new()
            };
            q::JS_FreeValue(self.ctx, e);
            Err(Exception { message, stack, interrupted })
        }
    }

    /// `String(v)`.
    pub fn string(&self, v: JSValue) -> String {
        unsafe {
            let mut len = 0usize;
            let p = q::JS_ToCStringLen2(self.ctx, &mut len, v, 0);
            if p.is_null() {
                // A value whose conversion throws: drop that exception.
                let e = q::JS_GetException(self.ctx);
                q::JS_FreeValue(self.ctx, e);
                return String::from("<unprintable>");
            }
            let s = String::from_utf8_lossy(std::slice::from_raw_parts(p.cast::<u8>(), len)).into_owned();
            q::JS_FreeCString(self.ctx, p);
            s
        }
    }

    pub fn new_string(&self, s: &str) -> JSValue {
        unsafe { JS_NewStringLen(self.ctx, s.as_ptr().cast(), s.len()) }
    }

    pub fn global(&self) -> JSValue {
        unsafe { q::JS_GetGlobalObject(self.ctx) }
    }

    /// A property of an object (a new reference).
    pub fn get(&self, obj: JSValue, name: &str) -> JSValue {
        let n = CString::new(name).unwrap();
        unsafe { q::JS_GetPropertyStr(self.ctx, obj, n.as_ptr()) }
    }

    /// Sets a property, taking ownership of `v`.
    pub fn set(&self, obj: JSValue, name: &str, v: JSValue) {
        let n = CString::new(name).unwrap();
        unsafe {
            q::JS_SetPropertyStr(self.ctx, obj, n.as_ptr(), v);
        }
    }

    pub fn function(&self, name: &str, f: unsafe extern "C" fn(*mut JSContext, JSValue, c_int, *mut JSValue) -> JSValue, args: c_int) -> JSValue {
        let n = CString::new(name).unwrap();
        unsafe { q::JS_NewCFunction2(self.ctx, Some(f), n.as_ptr(), args, q::JS_CFUNC_generic, 0) }
    }

    pub fn free(&self, v: JSValue) {
        unsafe { q::JS_FreeValue(self.ctx, v) }
    }

    pub fn memory(&self) -> MemoryUsage {
        let mut m = MemoryUsage::default();
        unsafe { JS_ComputeMemoryUsage(self.rt, &mut m) };
        m
    }

    pub fn collect(&self) {
        unsafe { q::JS_RunGC(self.rt) }
    }
}

impl Drop for Realm {
    fn drop(&mut self) {
        unsafe {
            q::JS_FreeContext(self.ctx);
            q::JS_FreeRuntime(self.rt);
        }
    }
}
