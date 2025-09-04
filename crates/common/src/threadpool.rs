use std::ffi::CString;

pub fn init_threadpool() {
    rayon::ThreadPoolBuilder::new()
        .start_handler(|idx| {
            let cstr = CString::new(format!("Rayon Worker {}", idx)).unwrap();
            unsafe { tracy_client::internal::set_thread_name(cstr.as_bytes().as_ptr()) }
        })
        .build_global()
        .unwrap();
}
