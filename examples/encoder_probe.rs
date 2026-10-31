use rk_mpp_rs::service::{Client, MppService};

fn main() {
    for client in [
        Client::Vepu1,
        Client::Vepu2,
        Client::Vepu2Jpeg,
        Client::JpegEnc,
        Client::RkvEnc,
    ] {
        let opened = MppService::open(client as u32);
        match opened {
            Ok(svc) => {
                let id = svc.hw_id();
                println!(
                    "client {:>2} {:<20} hw_id={}",
                    client as u32,
                    format!("{client:?}"),
                    match id {
                        Ok(id) => format!("0x{id:08x}"),
                        Err(e) => format!("error({e})"),
                    }
                );
            }
            Err(e) => {
                println!(
                    "client {:>2} {:<20} open error({e})",
                    client as u32,
                    format!("{client:?}")
                );
            }
        }
    }
}
