//! The guest's end of the channel through the QuickJS API: `ui.svcOpen`,
//! `ui.svcPoll` and `ui.svcSend` answered from the [`Interface`](crate::Interface)
//! in the process. PocketJS's device hosts carry those ops to a companion
//! over USB; a game that draws its own scene mounts these before the bundle
//! is evaluated, and the lines stay in the process.
use libquickjs_sys::*;

use crate::channel;

// libquickjs-sys omits this one; the linked QuickJS provides it (PocketJS's
// hosts declare it the same way).
extern "C" {
    fn JS_NewStringLen(ctx: *mut JSContext, text: *const u8, length: usize) -> JSValue;
}

unsafe fn argument<R>(ctx: *mut JSContext, argc: i32, argv: *mut JSValue, f: impl FnOnce(&str) -> R) -> Option<R> {
    if argc < 1 {
        return None;
    }
    let mut length: size_t = 0;
    let text = JS_ToCStringLen2(ctx, &mut length, *argv, 0);
    if text.is_null() {
        return None;
    }
    let result = core::str::from_utf8(core::slice::from_raw_parts(text as *const u8, length as usize)).ok().map(f);
    JS_FreeCString(ctx, text);
    result
}

unsafe extern "C" fn open(ctx: *mut JSContext, _this: JSValue, argc: i32, argv: *mut JSValue) -> JSValue {
    JS_NewBool(ctx, argument(ctx, argc, argv, |service| channel().open(service)).unwrap_or(false))
}

unsafe extern "C" fn poll(ctx: *mut JSContext, _this: JSValue, _argc: i32, _argv: *mut JSValue) -> JSValue {
    match channel().poll() {
        Some(line) => JS_NewStringLen(ctx, line.as_ptr(), line.len()),
        None => JS_UNDEFINED,
    }
}

unsafe extern "C" fn send(ctx: *mut JSContext, _this: JSValue, argc: i32, argv: *mut JSValue) -> JSValue {
    argument(ctx, argc, argv, |line| channel().receive(line));
    JS_UNDEFINED
}

/// Replaces the service ops on the guest's `ui` object. Call after the host
/// registered its own and before the bundle is evaluated.
///
/// # Safety
/// `ctx` and `global` are the live realm PocketJS's host created.
pub unsafe fn mount(ctx: *mut JSContext, global: JSValue) {
    let ui = JS_GetPropertyStr(ctx, global, c"ui".as_ptr());
    let ops: [(&core::ffi::CStr, unsafe extern "C" fn(*mut JSContext, JSValue, i32, *mut JSValue) -> JSValue, i32); 3] = [(c"svcOpen", open, 1), (c"svcPoll", poll, 0), (c"svcSend", send, 1)];
    for (name, function, arguments) in ops {
        let value = JS_NewCFunction2(ctx, Some(function), name.as_ptr(), arguments, JS_CFUNC_generic, 0);
        JS_SetPropertyStr(ctx, ui, name.as_ptr(), value);
    }
    JS_FreeValue(ctx, ui);
}
