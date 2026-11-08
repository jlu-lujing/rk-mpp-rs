use core::ffi::CStr;
use rk_mpp_rs_sys::{rk_mpp_api_version, rk_mpp_api_version_string};

fn main() {
    unsafe {
        let value = rk_mpp_api_version();
        let text = CStr::from_ptr(rk_mpp_api_version_string())
            .to_string_lossy()
            .into_owned();
        println!("rk-mpp-rs API {text} (0x{value:06x})");
    }
}
