use parking_lot::Mutex;

static MUTEX: Mutex<()> = Mutex::new(());

pub fn register_panic_hook() {
    let old_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |panic_info| {
        let _guard = MUTEX.lock();

        old_hook(panic_info);
    }));
}
