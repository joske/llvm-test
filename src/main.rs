use llvm_sys::{core::*, prelude::*, target::*, LLVMLinkage};
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::Write;
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

        let (builder, function) = add_function(context, module, "sum");
        // --- 5) Get the function's parameters & build "sum" = a0 + a1 ---
        let a0 = LLVMGetParam(function, 0);
        let a1 = LLVMGetParam(function, 1);
        let sum = LLVMBuildAdd(builder, a0, a1, cstr!("tmp"));

        // --- 6) Return the result ---
        LLVMBuildRet(builder, sum);

        let (builder, function) = add_function(context, module, "mul");
        // --- 5) Get the function's parameters & build "mul" = a0 * a1 ---
        let a0 = LLVMGetParam(function, 0);
        let a1 = LLVMGetParam(function, 1);
        let mul = LLVMBuildMul(builder, a0, a1, cstr!("tmp"));

        // --- 6) Return the result ---
        LLVMBuildRet(builder, mul);

        // --- 7) Print out the module as LLVM IR ---
        let ir_str_ptr = LLVMPrintModuleToString(module);
        let ir_str = CStr::from_ptr(ir_str_ptr);
        println!("Generated LLVM IR:\n{}", ir_str.to_string_lossy());
        File::create("output.ll")
            .unwrap()
            .write_all(ir_str.to_bytes())
            .unwrap();
        LLVMDisposeMessage(ir_str_ptr); // must free the string

        // --- 8) Clean up ---
        LLVMDisposeBuilder(builder);
        LLVMDisposeModule(module);
        LLVMContextDispose(context);
    }
}

fn add_function(
    context: *mut llvm_sys::LLVMContext,
    module: *mut llvm_sys::LLVMModule,
    name: &str,
) -> (*mut llvm_sys::LLVMBuilder, *mut llvm_sys::LLVMValue) {
    unsafe {
        add_polkavm_export_data_for_fn(module, context, name);
        add_polkavm_metadata(module, context, name, 2);

        let i32_type = LLVMInt32TypeInContext(context);
        let param_types = [i32_type, i32_type];
        let fn_type = LLVMFunctionType(
            i32_type,
            param_types.as_ptr() as *mut LLVMTypeRef,
            param_types.len() as u32,
            0, // not variadic
        );
        let fn_name = CString::new(name).unwrap();
        let function = LLVMAddFunction(module, fn_name.as_ptr(), fn_type);
        // Set the custom section
        let section_name = CString::new(format!(".text.polkavm_export.{}", name)).unwrap();
        LLVMSetSection(function, section_name.as_ptr());

        // --- 4) Create a basic block & a builder to emit instructions ---
        let entry_bb = LLVMAppendBasicBlockInContext(context, function, cstr!("entry"));
        let builder = LLVMCreateBuilderInContext(context);
        LLVMPositionBuilderAtEnd(builder, entry_bb);

        (builder, function)
    }
}

unsafe fn add_polkavm_metadata(
    module: LLVMModuleRef,
    context: LLVMContextRef,
    fn_name: &str,
    num_args: u8,
) -> LLVMValueRef {
    // Create the metadata
    let metadata_str = CString::new(fn_name).unwrap();
    let metadata_global = LLVMAddGlobal(
        module,
        LLVMArrayType2(
            LLVMInt8TypeInContext(context),
            metadata_str.as_bytes().len() as u64 + 1,
        ),
        CString::new("metadata_symbol").unwrap().as_ptr(),
    );
    LLVMSetSection(
        metadata_global,
        CString::new(format!(".rodata.{}_metadata", fn_name))
            .unwrap()
            .as_ptr(),
    );
    LLVMSetInitializer(
        metadata_global,
        LLVMConstString(
            metadata_str.as_ptr(),
            metadata_str.as_bytes().len() as u32,
            0,
        ),
    );

    // Define metadata structure
    let metadata_struct = LLVMStructType(
        [
            LLVMInt8Type(),
            LLVMInt32Type(),
            LLVMInt32Type(),
            LLVMPointerType(LLVMInt8Type(), 0),
            LLVMInt8Type(),
            LLVMInt8Type(),
        ]
        .as_mut_ptr(),
        6,
        0,
    );

    // Initialize metadata with values
    let mut metadata_values = [
        LLVMConstInt(LLVMInt8Type(), 1, 0),  // version
        LLVMConstInt(LLVMInt32Type(), 0, 0), // flags
        LLVMConstInt(LLVMInt32Type(), metadata_str.as_bytes().len() as u64, 0), // symbol length
        LLVMConstPointerCast(metadata_global, LLVMPointerType(LLVMInt8Type(), 0)), // pointer to symbol
        LLVMConstInt(LLVMInt8Type(), num_args as u64, 0),
        LLVMConstInt(LLVMInt8Type(), 1, 0),
    ];

    let metadata_constant = LLVMConstNamedStruct(metadata_struct, metadata_values.as_mut_ptr(), 6);
    let metadata = LLVMAddGlobal(
        module,
        metadata_struct,
        CString::new("metadata").unwrap().as_ptr(),
    );
    LLVMSetInitializer(metadata, metadata_constant);
    LLVMSetSection(
        metadata,
        CString::new(".polkavm_metadata").unwrap().as_ptr(),
    );

    metadata_global
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
