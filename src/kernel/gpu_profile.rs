//! web3d-M7 follow-up: GPU time per frame pass, for finding where a
//! frame goes (natively, with `TWE_GPU_PROFILE=1`).
//!
//! A timestamp is written into the frame's command encoder before each
//! render-graph pass and after the last; after the frame is submitted
//! they are read back (blocking on the GPU — a diagnostic, never on by
//! default) and averaged per pass. Every 60 frames the averages are
//! printed to stderr, largest first. Needs the adapter's
//! `TIMESTAMP_QUERY` and `TIMESTAMP_QUERY_INSIDE_ENCODERS` features;
//! without them profiling stays off.

use std::collections::HashMap;

/// Timestamps one frame can hold (passes + 1).
const MAX_STAMPS: u32 = 64;
/// Frames averaged per report.
const REPORT_EVERY: u32 = 60;

pub(crate) struct GpuProfile {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f32,
    /// This frame's pass labels, in timestamp order.
    labels: Vec<String>,
    totals: HashMap<String, f64>,
    frames: u32,
}

/// The features profiling needs, if `TWE_GPU_PROFILE` asks for it and
/// the adapter has them.
pub(crate) fn wanted_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let want = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    if cfg!(not(target_arch = "wasm32"))
        && std::env::var("TWE_GPU_PROFILE").is_ok_and(|v| v != "0")
        && adapter.features().contains(want)
    {
        want
    } else {
        wgpu::Features::empty()
    }
}

impl GpuProfile {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS) {
            return None;
        }
        let size = u64::from(MAX_STAMPS) * 8;
        Some(GpuProfile {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("twe-kernel gpu profile"),
                ty: wgpu::QueryType::Timestamp,
                count: MAX_STAMPS,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("twe-kernel gpu profile resolve"),
                size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("twe-kernel gpu profile readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            period_ns: queue.get_timestamp_period(),
            labels: Vec::new(),
            totals: HashMap::new(),
            frames: 0,
        })
    }

    /// Mark the start of pass `label` (and the end of the one before).
    pub fn mark(&mut self, encoder: &mut wgpu::CommandEncoder, label: String) {
        let i = self.labels.len() as u32;
        if i + 1 < MAX_STAMPS {
            encoder.write_timestamp(&self.queries, i);
            self.labels.push(label);
        }
    }

    /// Close the frame: the last timestamp, and the copy to read back.
    pub fn finish(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let n = self.labels.len() as u32;
        if n == 0 {
            return;
        }
        encoder.write_timestamp(&self.queries, n);
        encoder.resolve_query_set(&self.queries, 0..n + 1, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, u64::from(n + 1) * 8);
    }

    /// After the frame's submit: read the timestamps back (blocking)
    /// and add each pass's time to the running totals.
    pub fn collect(&mut self, device: &wgpu::Device) {
        let labels = std::mem::take(&mut self.labels);
        if labels.is_empty() {
            return;
        }
        let bytes = (labels.len() as u64 + 1) * 8;
        let slice = self.readback.slice(0..bytes);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        if let Ok(data) = slice.get_mapped_range() {
            let stamps: &[u64] = bytemuck::cast_slice(&data);
            for (i, label) in labels.iter().enumerate() {
                let ns = stamps[i + 1].saturating_sub(stamps[i]) as f64 * f64::from(self.period_ns);
                *self.totals.entry(label.clone()).or_default() += ns;
            }
        }
        self.readback.unmap();
        self.frames += 1;
        if self.frames == REPORT_EVERY {
            let mut rows: Vec<(String, f64)> = self
                .totals
                .drain()
                .map(|(k, v)| (k, v / 1e6 / f64::from(REPORT_EVERY)))
                .collect();
            rows.sort_by(|a, b| b.1.total_cmp(&a.1));
            let total: f64 = rows.iter().map(|r| r.1).sum();
            let list: Vec<String> = rows.iter().map(|(k, v)| format!("{k} {v:.2}")).collect();
            eprintln!("gpu profile: {total:.2} ms/frame: {}", list.join(", "));
            self.frames = 0;
        }
    }
}
