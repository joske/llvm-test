use llvm_sys::{bit_writer::LLVMWriteBitcodeToFile, core::*, prelude::*, target::*, LLVMLinkage};
use std::{
    ffi::{CStr, CString},
    fs::File,
    io::Write,
};

#[macro_export]
macro_rules! cstr {
    ($lit:expr) => {{
        // We use `concat!` to join the literal and a "\0" at compile time.
        // Then `.as_ptr()` to get a pointer to the bytes,
        // and cast to *const i8 (LLVM-style C-char).
        concat!($lit, "\0").as_ptr() as *const i8
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

        create_export_data(context, module);

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
        LLVMWriteBitcodeToFile(module, b"output.ll\0".as_ptr().cast());
        LLVMDisposeMessage(ir_str_ptr); // must free the string

        // --- 8) Clean up ---
        LLVMDisposeBuilder(builder);
        LLVMDisposeModule(module);
        LLVMContextDispose(context);
    }
}

/// Creates a global array of 9 bytes: [1, 0, 0, 0, 0, 0, 0, 0, 0],
/// and places it in the .polkavm_exports section.
unsafe fn create_export_data(context: LLVMContextRef, module: LLVMModuleRef) {
    // 1) Define the array type: [9 x i8]
    let num_bytes = 9u64;
    let i8_ty = LLVMInt8TypeInContext(context);
    let array_ty = LLVMArrayType2(i8_ty, num_bytes);

    // 2) Build a constant initializer: [1, 0, 0, 0, 0, 0, 0, 0, 0]
    let data = [1, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut values = Vec::with_capacity(data.len());
    for &b in &data {
        let c = LLVMConstInt(i8_ty, b as u64, 0); // 0 => unsigned
        values.push(c);
    }
    let init_array = LLVMConstArray2(i8_ty, values.as_mut_ptr(), values.len() as u64);

    // 3) Create a new global with that array type
    let global_name = cstr!("my_export_data");
    let global = LLVMAddGlobal(module, array_ty, global_name);

    // 4) Set the initializer
    LLVMSetInitializer(global, init_array);

    // 5) Place it in the custom section
    let section_name = cstr!(".polkavm_exports");
    LLVMSetSection(global, section_name);

    // 6) Adjust linkage to ensure it isn't optimized away
    //    - ExternalLinkage means it's "public"; you could also use InternalLinkage
    //      if you prefer, but then you'll want to ensure it's not removed by LTO.
    LLVMSetLinkage(global, LLVMLinkage::LLVMExternalLinkage);
}
