use media_core::{Av1anResourceEstimate, Av1anResourceRequest, VideoEncoder};

fn memory() -> (Option<u32>, Option<u32>) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // The OS fills this fixed-size structure; no pointers escape the call.
        let mut state: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        state.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut state) } != 0 {
            let mib = |bytes: u64| Some((bytes / (1024 * 1024)).min(u64::from(u32::MAX)) as u32);
            return (mib(state.ullTotalPhys), mib(state.ullAvailPhys));
        }
    }
    (None, None)
}

pub fn estimate_av1an_resources(request: Av1anResourceRequest) -> Av1anResourceEstimate {
    let processors = std::thread::available_parallelism().map_or(1, |n| n.get().min(4096) as u32);
    let (total, available) = memory();
    estimate(request, processors, total, available)
}

fn estimate(
    r: Av1anResourceRequest,
    processors: u32,
    total: Option<u32>,
    available: Option<u32>,
) -> Av1anResourceEstimate {
    let pixels = |w: u32, h: u32| f64::from(w.min(32768)) * f64::from(h.min(32768)) / 1_000_000.0;
    let source_mp = pixels(r.source_width, r.source_height);
    let output_mp = pixels(r.output_width, r.output_height);
    // Approximate 10-bit encoder/decoder working sets. These are advisory estimates,
    // not reservations or measured peak memory for the selected tool build.
    let slope = match r.encoder {
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => 605.0,
        VideoEncoder::X264 => 397.0,
        _ => 400.0,
    };
    let filters = if r.filtered {
        60.0 + source_mp * 13.0
            + if r.float_filter {
                source_mp * 42.0
            } else {
                0.0
            }
    } else {
        0.0
    };
    let per_worker_mib = (150.0 + output_mp * slope + source_mp * 51.0 + filters).ceil() as u32;
    let workers = r.workers.clamp(1, 64);
    let estimated_memory_mib = per_worker_mib.saturating_mul(u32::from(workers));
    let usable = total
        .map(|t| t.saturating_sub(2048.max(t / 5)))
        .map(|budget| available.map_or(budget, |a| budget.min(a)));
    let memory_workers = usable
        .map(|m| ((f64::from(m) / (f64::from(per_worker_mib) * 1.25)).floor() as u32).clamp(1, 64));
    let baseline_workers = (processors * 2).div_ceil(5).clamp(2, 32);
    let cpu_workers = baseline_workers;
    let cpu_workers = if r.encoder.is_svt() {
        cpu_workers.saturating_sub(2).max(1)
    } else {
        cpu_workers
    };
    let suggested_workers = memory_workers.map_or(cpu_workers, |n| n.min(cpu_workers)) as u8;
    let thread_budget = (f64::from(processors) * 0.8).round();
    let suggested_threads = (thread_budget / f64::from(baseline_workers))
        .round()
        .clamp(2.0, 16.0) as u8;
    let warning = usable.filter(|budget| f64::from(estimated_memory_mib) * 1.25 > f64::from(*budget)).map(|_| {
        format!("Estimated encoder memory is {:.1} GiB for {workers} workers. Try {} workers to leave memory headroom. Decoder caches, presets and other applications can change actual use; this estimate does not block encoding.", f64::from(estimated_memory_mib)/1024.0, memory_workers.unwrap_or(1))
    });
    Av1anResourceEstimate {
        logical_processors: processors,
        total_memory_mib: total,
        available_memory_mib: available,
        per_worker_mib,
        estimated_memory_mib,
        suggested_workers,
        suggested_threads,
        suggested_scene_slices: (processors / 2).clamp(1, 8) as u8,
        warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Av1anResourceRequest {
        Av1anResourceRequest {
            encoder: VideoEncoder::SvtAv1,
            source_width: 3840,
            source_height: 2160,
            output_width: 1920,
            output_height: 1080,
            workers: 8,
            filtered: true,
            float_filter: false,
        }
    }
    #[test]
    fn memory_pressure_warns_without_rewriting_the_request() {
        let low = estimate(request(), 16, Some(32768), Some(4096));
        assert!(low.warning.is_some());
        assert_eq!(low.estimated_memory_mib, low.per_worker_mib * 8);
        assert!(low.suggested_workers < 8);
        let mut smaller = request();
        smaller.workers = 1;
        assert!(
            estimate(smaller, 16, Some(32768), Some(24000))
                .warning
                .is_none()
        );
    }
    #[test]
    fn unknown_memory_remains_unknown_and_float_filters_increase_estimate() {
        let baseline = estimate(request(), 16, None, None);
        assert_eq!(baseline.total_memory_mib, None);
        assert_eq!(baseline.warning, None);
        let mut float = request();
        float.float_filter = true;
        assert!(estimate(float, 16, None, None).per_worker_mib > baseline.per_worker_mib);
        let mut smaller = request();
        smaller.output_width = 640;
        smaller.output_height = 360;
        assert!(estimate(smaller, 16, None, None).per_worker_mib < baseline.per_worker_mib);
    }

    #[test]
    fn recommendations_balance_worker_and_thread_budgets() {
        let mut source = request();
        source.source_width = 1280;
        source.source_height = 720;
        let svt = estimate(source, 16, Some(65536), Some(60000));
        assert_eq!((svt.suggested_workers, svt.suggested_threads), (5, 2));
        source.encoder = VideoEncoder::X264;
        let x264 = estimate(source, 16, Some(65536), Some(60000));
        assert_eq!((x264.suggested_workers, x264.suggested_threads), (7, 2));
    }
}
