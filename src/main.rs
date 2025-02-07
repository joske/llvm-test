use llvm_sys::{core::*, prelude::*, target::*, LLVMLinkage, LLVMUnnamedAddr};
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
    // Create the metadata symbol
    let i8_type = LLVMInt8TypeInContext(context);
    let array_ty = LLVMArrayType2(i8_type, fn_name.len() as u64);

    let mut struct_field_types = [array_ty];
    let struct_ty = LLVMStructType(struct_field_types.as_mut_ptr(), 1, 0);
    let text = CString::new(fn_name).unwrap();
    let const_array = LLVMConstStringInContext2(context, text.as_ptr(), text.as_bytes().len(), 1);
    let mut struct_values = [const_array];
    let const_struct = LLVMConstStruct(struct_values.as_mut_ptr(), 1, 0);
    let hashed = hash_string(fn_name);
    let metadata_str = CString::new(format!("alloc_{}", hashed)).unwrap();
    let metadata_global = LLVMAddGlobal(module, struct_ty, metadata_str.as_ptr());
    LLVMSetInitializer(metadata_global, const_struct);
    LLVMSetLinkage(metadata_global, LLVMLinkage::LLVMPrivateLinkage);
    LLVMSetUnnamedAddress(metadata_global, LLVMUnnamedAddr::LLVMGlobalUnnamedAddr);
    LLVMSetAlignment(metadata_global, 1);
    LLVMSetGlobalConstant(metadata_global, 1);
    LLVMSetSection(
        metadata_global,
        CString::new(format!(".rodata..Lalloc_{}", hashed))
            .unwrap()
            .as_ptr(),
    );

    // Define metadata data
    let i8_type = LLVMInt8TypeInContext(context);
    let ptr_type = LLVMPointerType(i8_type, 0);
    let arr9_type = LLVMArrayType2(i8_type, 9);
    let arr2_type = LLVMArrayType2(i8_type, 2);
    let mut field_types = [arr9_type, ptr_type, arr2_type];
    let metadata_struct_ty = LLVMStructType(field_types.as_mut_ptr(), 3, 1);
    let mut byte_consts_field0 = Vec::with_capacity(9);
    // version
    byte_consts_field0.push(LLVMConstInt(i8_type, 1, 0));
    // flags -> 0
    for _ in 0..4 {
        byte_consts_field0.push(LLVMConstInt(i8_type, 0, 0));
    }
    // function name length
    let bytes_field0 = (fn_name.len() as u32).to_le_bytes();
    for &b in &bytes_field0 {
        byte_consts_field0.push(LLVMConstInt(i8_type, b as u64, 0));
    }
    // pointer to the symbol
    let const_arr9 = LLVMConstArray2(i8_type, byte_consts_field0.as_mut_ptr(), 9);
    let const_ptr = LLVMConstPointerCast(metadata_global, ptr_type);
    // number of input and output args
    let bytes_field2: [u64; 2] = [num_args as u64, 1];
    let mut byte_consts_field2 = Vec::with_capacity(2);
    for &b in &bytes_field2 {
        byte_consts_field2.push(LLVMConstInt(i8_type, b, 0));
    }
    let const_arr2 = LLVMConstArray2(i8_type, byte_consts_field2.as_mut_ptr(), 2);
    let mut metadata_fields = [const_arr9, const_ptr, const_arr2];
    let metadata_const = LLVMConstStruct(metadata_fields.as_mut_ptr(), 3, 1);

    let mangled = format!(
        "_ZN{}{}{}{}8METADATA17h{}E",
        module_name.len(),
        module_name,
        fn_name.len(),
        fn_name,
        hash_string("METADATA")
    );
    let metadata = LLVMAddGlobal(
        module,
        metadata_struct_ty,
        CString::new(mangled.clone()).unwrap().as_ptr(),
    );
    LLVMSetInitializer(metadata, metadata_const);
    LLVMSetAlignment(metadata, 1);
    LLVMSetGlobalConstant(metadata, 1);
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
