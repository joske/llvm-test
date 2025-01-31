use llvm_sys::{bit_writer::LLVMWriteBitcodeToFile, core::*, prelude::*, target::*, LLVMLinkage};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

#[macro_export]
macro_rules! cstr {
    ($lit:expr) => {{
        // We use `concat!` to join the literal and a "\0" at compile time.
        // Then `.as_ptr()` to get a pointer to the bytes,
        // and cast to *const i8 (LLVM-style C-char).
        concat!($lit, "\0").as_ptr() as *const c_char
    }};
}

fn main() {
    unsafe {
        // --- 1) Initialize LLVM (important for target-specific features) ---
        LLVM_InitializeNativeTarget();
        LLVM_InitializeNativeAsmPrinter();
        LLVM_InitializeNativeAsmParser();

        // --- 2) Create a Context & Module ---
        let context = LLVMContextCreate();
        let module_name = cstr!("my_module");
        let module = LLVMModuleCreateWithNameInContext(module_name, context);

        add_polkavm_export_data_for_fn(module, context, "sum");
        add_polkavm_metadata(module, context, "sum", 2);

        // --- 3) Create the signature of our function: i32 sum(i32, i32) ---
        let i32_type = LLVMInt32TypeInContext(context);
        let param_types = [i32_type, i32_type];
        let fn_type = LLVMFunctionType(
            i32_type,
            param_types.as_ptr() as *mut LLVMTypeRef,
            param_types.len() as u32,
            0, // not variadic
        );
        let fn_name = cstr!("sum");
        let function = LLVMAddFunction(module, fn_name, fn_type);
        // Set the custom section
        let section_name = CString::new(".text.polkavm_export.sum").unwrap();
        LLVMSetSection(function, section_name.as_ptr());

        // --- 4) Create a basic block & a builder to emit instructions ---
        let entry_bb = LLVMAppendBasicBlockInContext(context, function, cstr!("entry"));
        let builder = LLVMCreateBuilderInContext(context);
        LLVMPositionBuilderAtEnd(builder, entry_bb);

        // --- 5) Get the function's parameters & build "sum" = a0 + a1 ---
        let a0 = LLVMGetParam(function, 0);
        let a1 = LLVMGetParam(function, 1);
        let sum = LLVMBuildAdd(builder, a0, a1, cstr!("sumtmp"));

        // --- 6) Return the result ---
        LLVMBuildRet(builder, sum);

        // --- 7) Print out the module as LLVM IR ---
        let ir_str_ptr = LLVMPrintModuleToString(module);
        let ir_str = CStr::from_ptr(ir_str_ptr);
        println!("Generated LLVM IR:\n{}", ir_str.to_string_lossy());
        LLVMWriteBitcodeToFile(module, "output.ll".as_ptr().cast());
        LLVMDisposeMessage(ir_str_ptr); // must free the string

        // --- 8) Clean up ---
        LLVMDisposeBuilder(builder);
        LLVMDisposeModule(module);
        LLVMContextDispose(context);
    }
}

unsafe fn add_polkavm_metadata(
    module: LLVMModuleRef,
    context: LLVMContextRef,
    fn_name: &str,
    args: u8,
) -> LLVMValueRef {
    let i8_type = LLVMInt8TypeInContext(context);
    let i32_type = LLVMInt32TypeInContext(context);
    let array_type = LLVMArrayType2(i8_type, 15);

    // We'll name the global like "<fn_name>_export_data" or something
    let global_name = CString::new(format!("{}_export_data", fn_name)).unwrap();
    let global = LLVMAddGlobal(module, array_type, global_name.as_ptr());

    // Place it in the .polkavm_exports section
    LLVMSetSection(global, cstr!(".polkavm_metadata"));

    // Initialize first byte = 1, next 8 = 0
    let mut bytes = Vec::with_capacity(9);
    bytes.push(LLVMConstInt(i8_type, 1, 0));
    for _ in 0..3 {
        bytes.push(LLVMConstInt(i8_type, 0, 0));
    }
    // symbol name length
    bytes.push(LLVMConstInt(i32_type, fn_name.len() as u64, 0));
    // pointer seems to be 0
    for _ in 0..3 {
        bytes.push(LLVMConstInt(i8_type, 0, 0));
    }
    // input
    bytes.push(LLVMConstInt(i8_type, args as u64, 0));
    // output
    bytes.push(LLVMConstInt(i8_type, 1, 0));

    let init_array = LLVMConstArray2(i8_type, bytes.as_mut_ptr(), bytes.len() as u64);
    LLVMSetInitializer(global, init_array);

    // Possibly mark the global as internal, so the symbol won't clash
    LLVMSetLinkage(global, LLVMLinkage::LLVMExternalLinkage);

    global
}

/// Creates a 9-byte global in `.polkavm_exports`.
///  Byte[0] = 1, Byte[1..8] = 0.
unsafe fn add_polkavm_export_data_for_fn(
    module: LLVMModuleRef,
    context: LLVMContextRef,
    fn_name: &str,
) -> LLVMValueRef {
    let i8_type = LLVMInt8TypeInContext(context);
    let array_type = LLVMArrayType2(i8_type, 9);

    // We'll name the global like "<fn_name>_export_data" or something
    let global_name = CString::new(format!("{}_export_data", fn_name)).unwrap();
    let global = LLVMAddGlobal(module, array_type, global_name.as_ptr());

    // Place it in the .polkavm_exports section
    LLVMSetSection(global, cstr!(".polkavm_exports"));

    // Initialize first byte = 1, next 8 = 0
    let mut bytes = Vec::with_capacity(9);
    bytes.push(LLVMConstInt(i8_type, 1, 0));
    for _ in 0..8 {
        bytes.push(LLVMConstInt(i8_type, 0, 0));
    }
    let init_array = LLVMConstArray2(i8_type, bytes.as_mut_ptr(), bytes.len() as u64);
    LLVMSetInitializer(global, init_array);

    // Possibly mark the global as internal, so the symbol won't clash
    LLVMSetLinkage(global, LLVMLinkage::LLVMExternalLinkage);

    global
}
