use gpui_wgpu::mesa_breaks_storage_reads;

#[test]
fn only_llvmpipe_with_llvm_22_or_later_uses_the_texture() {
    assert!(mesa_breaks_storage_reads("llvmpipe (LLVM 22.1.1, 128 bits)"));
    assert!(mesa_breaks_storage_reads("llvmpipe (LLVM 23.0.0, 256 bits)"));
    assert!(!mesa_breaks_storage_reads("llvmpipe (LLVM 20.1.2, 128 bits)"));
    assert!(!mesa_breaks_storage_reads("llvmpipe (LLVM unknown, 128 bits)"));
    assert!(!mesa_breaks_storage_reads("virgl (Apple M5 Pro)"));
    assert!(!mesa_breaks_storage_reads("AMD Radeon (LLVM 22.1.1)"));
}
