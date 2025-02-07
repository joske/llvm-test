use llvm_sys::{core::*, prelude::*, target::*, LLVMLinkage};
use std::ffi::{CStr, CString};
use std::fs::File;
use std::hash::DefaultHasher;
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
        let module_name = "my_module";
        let module_name_c = cstr!("my_module");
        let module = LLVMModuleCreateWithNameInContext(module_name_c, context);
        LLVMSetDataLayout(module, cstr!("e-m:e-p:32:32-i64:64-n32-S32"));
        LLVMSetTarget(module, cstr!("riscv"));

        let mut asm = String::with_capacity(100);
        let (builder, function) = add_function(context, module, module_name, "sum", &mut asm);
        // --- 5) Get the function's parameters & build "sum" = a0 + a1 ---
        let a0 = LLVMGetParam(function, 0);
        let a1 = LLVMGetParam(function, 1);
        let sum = LLVMBuildAdd(builder, a0, a1, cstr!("tmp"));

        // --- 6) Return the result ---
        LLVMBuildRet(builder, sum);

        let (builder, function) = add_function(context, module, module_name, "sub", &mut asm);
        // --- 5) Get the function's parameters & build "mul" = a0 - a1 ---
        let a0 = LLVMGetParam(function, 0);
        let a1 = LLVMGetParam(function, 1);
        let sub = LLVMBuildSub(builder, a0, a1, cstr!("tmp"));

        // --- 6) Return the result ---
        LLVMBuildRet(builder, sub);

        LLVMSetModuleInlineAsm2(module, asm.as_ptr(), asm.len());
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
    module_name: &str,
    fn_name: &str,
    asm: &mut String,
) -> (*mut llvm_sys::LLVMBuilder, *mut llvm_sys::LLVMValue) {
    unsafe {
        let i32_type = LLVMInt32TypeInContext(context);
        let param_types = [i32_type, i32_type];
        let fn_type = LLVMFunctionType(
            i32_type,
            param_types.as_ptr() as *mut LLVMTypeRef,
            param_types.len() as u32,
            0, // not variadic
        );
        let hash = hash_string(format!("{}::{}", module_name, fn_name).as_str());
        let mangled = format!(
            "_ZN{}{}{}{}17h{}E",
            module_name.len(),
            module_name,
            fn_name.len(),
            fn_name,
            hash
        );
        let fn_name_c = CString::new(mangled.clone()).unwrap();
        let function = LLVMAddFunction(module, fn_name_c.as_ptr(), fn_type);
        // Set the custom section
        let section_name = CString::new(format!(".text.polkavm_export.{}", fn_name)).unwrap();
        LLVMSetSection(function, section_name.as_ptr());
        LLVMSetLinkage(function, LLVMLinkage::LLVMInternalLinkage);

        add_polkavm_metadata(
            module,
            context,
            module_name,
            fn_name,
            mangled.as_str(),
            2,
            asm,
        );

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
    module_name: &str,
    fn_name: &str,
    mangled_fn_name: &str,
    num_args: u8,
    asm: &mut String,
) {
    // Create the metadata
    let mangled = format!(
        "_ZN{}{}{}{}8METADATA17h{}E",
        module_name.len(),
        module_name,
        fn_name.len(),
        fn_name,
        hash_string("METADATA")
    );
    let metadata_str = CString::new(fn_name).unwrap();
    let metadata_global = LLVMAddGlobal(
        module,
        LLVMArrayType2(
            LLVMInt8TypeInContext(context),
            metadata_str.as_bytes().len() as u64 + 1,
        ),
        metadata_str.as_ptr(),
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
    LLVMSetLinkage(metadata_global, LLVMLinkage::LLVMInternalLinkage);

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
        LLVMConstInt(LLVMInt32Type(), 1, 0), // flags
        LLVMConstInt(LLVMInt32Type(), metadata_str.as_bytes().len() as u64, 0), // symbol length
        LLVMConstPointerCast(metadata_global, LLVMPointerType(LLVMInt8Type(), 0)), // pointer to symbol
        LLVMConstInt(LLVMInt8Type(), num_args as u64, 0),
        LLVMConstInt(LLVMInt8Type(), 1, 0),
    ];

    let metadata_constant = LLVMConstStruct(metadata_values.as_mut_ptr(), 6, 0);
    let metadata = LLVMAddGlobal(
        module,
        metadata_struct,
        CString::new(mangled.clone()).unwrap().as_ptr(),
    );
    LLVMSetInitializer(metadata, metadata_constant);
    LLVMSetAlignment(metadata, 0);
    LLVMSetSection(
        metadata,
        CString::new(".polkavm_metadata").unwrap().as_ptr(),
    );
    LLVMSetLinkage(metadata, LLVMLinkage::LLVMInternalLinkage);

    asm.push_str(
        format!(
        ".pushsection .polkavm_exports,\"R\",@note\n.byte 1\n.4byte {}\n.4byte {}\n.popsection\n",
        mangled, mangled_fn_name
    )
        .as_str(),
    );
}

fn hash_string(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    let hash = hasher.finish();
    hex::encode(hash.to_be_bytes())
}
