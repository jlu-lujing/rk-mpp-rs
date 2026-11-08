use std::os::fd::AsRawFd;

use rk_mpp_rs::service::dma_heap::DmaBuf;

#[test]
#[ignore = "requires /dev/dma_heap/system"]
fn duplicate_and_import_preserve_dma_heap_bytes() {
    let mut original = DmaBuf::alloc("system", 4096, false).unwrap();
    original.sync_write_begin().unwrap();
    for (index, byte) in original.as_mut_slice().iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(37);
    }
    original.sync_write_end().unwrap();

    let duplicate = original.dup_fd().unwrap();
    let imported = DmaBuf::import_fd(duplicate.as_raw_fd(), original.len(), false).unwrap();
    imported.sync_read_begin().unwrap();
    assert_eq!(imported.as_slice(), original.as_slice());
    imported.sync_read_end().unwrap();

    let cloned = original.try_clone().unwrap();
    cloned.sync_read_begin().unwrap();
    assert_eq!(cloned.as_slice(), original.as_slice());
    cloned.sync_read_end().unwrap();
}
