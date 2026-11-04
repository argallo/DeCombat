//! A REAL GPU device loss (fix-hang, CLI-460): a fragment pass that runs far longer than the Windows GPU time-out
//! (TDR, 2 s by default) makes the driver reset the GPU; the next submit or poll on the device meets the loss.
//! wgpu-core 25.0.2 deadlocks there (crate::gpulost); with `gpulost::watch` the call must come back as `Err(GpuReset)`
//! within the time limit instead.
//!
//! MANUAL: the test resets the machine's display driver (the screen goes black for a moment), so it is ignored and
//! runs only with `DC_GPU_LOSS_TEST=1 cargo test -p dc-client --test gpu_loss -- --ignored`.
// ORIGINAL: our own test of the wgpu device-loss path (fix-hang); no 2009 behaviour

use dc_client::gpulost;
use dc_render::{AdapterChoice, GpuContext};
use std::time::Duration;

/// Submits the long pass and then keeps submitting and polling until the device reports the loss.
fn lose_the_device() -> Result<(), gpulost::GpuReset> {
    let ctx = GpuContext::headless(AdapterChoice::Auto).expect("a GPU");
    eprintln!("gpu_loss: adapter {:?}", ctx.info);
    // DC_GPU_LOSS_CONTROL=1: without the watch (the wgpu-core deadlock is expected; the control run of the report)
    if std::env::var("DC_GPU_LOSS_CONTROL").ok().as_deref() != Some("1") {
        gpulost::watch(&ctx.device);
    }
    let (device, queue) = (&ctx.device, &ctx.queue);
    let r = gpulost::guard(|| {
        for round in 0..40 {
            gpulost::provoke(device, queue);
            let _ = device.poll(wgpu::PollType::Poll);
            eprintln!("gpu_loss: round {round} submitted");
            std::thread::sleep(Duration::from_millis(500));
        }
    });
    // leaked on purpose, on the loss path too: dropping objects of a lost device can deadlock (crate::gpulost)
    std::mem::forget(ctx);
    r
}

#[test]
#[ignore = "resets the display driver: DC_GPU_LOSS_TEST=1 and --ignored"]
fn a_lost_device_returns_instead_of_deadlocking() {
    if std::env::var("DC_GPU_LOSS_TEST").ok().as_deref() != Some("1") {
        eprintln!("gpu_loss: DC_GPU_LOSS_TEST=1 not set; skipped");
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = lose_the_device();
        let _ = tx.send(r);
    });
    match rx.recv_timeout(Duration::from_secs(90)) {
        Ok(r) => {
            assert_eq!(
                r,
                Err(gpulost::GpuReset),
                "the device was not lost within 40 rounds"
            );
            assert!(gpulost::is_lost());
            assert!(gpulost::losses() >= 1);
        }
        Err(_) => panic!("the lost device did not come back within 90 s (the wgpu-core deadlock)"),
    }
}
