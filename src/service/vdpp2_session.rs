//! RK3576 VDPP2 client 29 session and DMA-BUF ownership.

use super::dma_heap::DmaBuf;
use super::{Batch, Client, MppService, RegOffsetInfo};
use crate::hal::vdpp2::{self, Vdpp2Config, Vdpp2Histogram, Vdpp2Job, Vdpp2Registers};

pub const VDPP_CLIENT: u32 = Client::Vdpp as u32;
pub const HISTOGRAM_BYTES: usize = vdpp2::HIST_PACKED_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeReport {
    pub present: bool,
    pub binds: bool,
    pub reg_offset: bool,
    pub poll: bool,
    pub hw_id: Option<u32>,
    pub open_errno: Option<i32>,
}

pub fn probe() -> ProbeReport {
    let mut report = ProbeReport {
        present: false,
        binds: false,
        reg_offset: false,
        poll: false,
        hw_id: None,
        open_errno: None,
    };
    match MppService::open(VDPP_CLIENT) {
        Ok(service) => {
            report.binds = true;
            if let Ok(mask) = service.probe_hw_support() {
                report.present = (mask >> VDPP_CLIENT) & 1 == 1;
            }
            report.reg_offset = service.cmd_supported(super::MPP_CMD_SET_REG_ADDR_OFFSET);
            report.poll = service.cmd_supported(super::MPP_CMD_POLL_HW_FINISH);
            report.hw_id = service.hw_id().ok();
        }
        Err(error) => report.open_errno = error.raw_os_error(),
    }
    report
}

pub struct Vdpp2Processor {
    service: MppService,
    config: Vdpp2Config,
    src: DmaBuf,
    dst: DmaBuf,
    dst_c: Option<DmaBuf>,
    histogram: DmaBuf,
}

impl Vdpp2Processor {
    pub fn open(config: Vdpp2Config) -> Result<Self, String> {
        config.validate()?;
        crate::service::soc::require_video_post_process(
            crate::service::soc::VideoPostProcessor::Vdpp2,
        )
        .map_err(|error| format!("VDPP2 SoC check: {error}"))?;
        let service = MppService::open(VDPP_CLIENT)
            .map_err(|error| format!("open VDPP2 client 29: {error}"))?;
        let heap = std::env::var("VDPP_HEAP").unwrap_or_else(|_| "system".to_string());
        let src_bytes = config
            .src_width_vir
            .checked_mul(config.src_height_vir)
            .and_then(|luma| luma.checked_mul(3)?.checked_div(2))
            .ok_or_else(|| "VDPP2 source size overflow".to_string())?;
        let dst_bytes = match config.dst_format {
            vdpp2::Vdpp2Format::Yuv420 => config
                .dst_width_vir
                .checked_mul(config.dst_height_vir)
                .and_then(|luma| luma.checked_mul(3)?.checked_div(2)),
            vdpp2::Vdpp2Format::Yuv444 => config
                .dst_width_vir
                .checked_mul(config.dst_height_vir)
                .and_then(|luma| luma.checked_mul(3)),
        }
        .ok_or_else(|| "VDPP2 destination size overflow".to_string())?;
        let dst_c_bytes = if config.yuv_out_diff {
            Some(
                config
                    .dst_c_width_vir
                    .checked_mul(config.dst_c_height_vir)
                    .and_then(|chroma| chroma.checked_mul(2))
                    .ok_or_else(|| "VDPP2 chroma destination size overflow".to_string())?,
            )
        } else {
            None
        };
        let dst_c = dst_c_bytes
            .map(|bytes| DmaBuf::alloc(&heap, bytes, false))
            .transpose()
            .map_err(|error| format!("allocate VDPP2 chroma destination: {error}"))?;
        Ok(Self {
            service,
            config,
            src: DmaBuf::alloc(&heap, src_bytes, false)
                .map_err(|error| format!("allocate VDPP2 source: {error}"))?,
            dst: DmaBuf::alloc(&heap, dst_bytes, false)
                .map_err(|error| format!("allocate VDPP2 destination: {error}"))?,
            dst_c,
            histogram: DmaBuf::alloc(&heap, HISTOGRAM_BYTES, false)
                .map_err(|error| format!("allocate VDPP2 histogram: {error}"))?,
        })
    }

    pub fn config(&self) -> &Vdpp2Config {
        &self.config
    }

    pub fn src_frame_mut(&mut self) -> &mut DmaBuf {
        &mut self.src
    }

    pub fn dst_frame(&self) -> &[u8] {
        self.dst.as_slice()
    }

    pub fn dst_chroma_frame(&self) -> Option<&[u8]> {
        self.dst_c.as_ref().map(DmaBuf::as_slice)
    }

    pub fn dst_frame_compact(&self) -> Result<Vec<u8>, String> {
        let width = self.config.dst_width;
        let height = self.config.dst_height;
        let stride = self.config.dst_width_vir;
        let chroma_bytes = self
            .config
            .dst_c_width
            .checked_mul(self.config.dst_c_height)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| "VDPP2 compact chroma size overflow".to_string())?;
        let capacity = width
            .checked_mul(height)
            .and_then(|luma| luma.checked_add(chroma_bytes))
            .ok_or_else(|| "VDPP2 compact output size overflow".to_string())?;
        let mut out = Vec::with_capacity(capacity);
        append_rows(&mut out, self.dst.as_slice(), 0, stride, width, height)?;
        if let Some(dst_c) = &self.dst_c {
            let chroma_stride = self
                .config
                .dst_c_width_vir
                .checked_mul(2)
                .ok_or_else(|| "VDPP2 compact chroma stride overflow".to_string())?;
            append_rows(
                &mut out,
                dst_c.as_slice(),
                0,
                chroma_stride,
                self.config.dst_c_width * 2,
                self.config.dst_c_height,
            )?;
            return Ok(out);
        }

        let src = self.dst.as_slice();
        let uv_offset = stride
            .checked_mul(self.config.dst_height_vir)
            .ok_or_else(|| "VDPP2 destination luma size overflow".to_string())?;
        match self.config.dst_format {
            vdpp2::Vdpp2Format::Yuv420 => {
                append_rows(&mut out, src, uv_offset, stride, width, height.div_ceil(2))?;
            }
            vdpp2::Vdpp2Format::Yuv444 => {
                let uv_stride = stride
                    .checked_mul(2)
                    .ok_or_else(|| "VDPP2 compact UV stride overflow".to_string())?;
                append_rows(&mut out, src, uv_offset, uv_stride, width * 2, height)?;
            }
        }
        Ok(out)
    }

    pub fn histogram(&self) -> &[u8] {
        &self.histogram.as_slice()[..HISTOGRAM_BYTES]
    }

    pub fn parsed_histogram(&self) -> Result<Vdpp2Histogram, String> {
        Vdpp2Histogram::parse(self.histogram())
    }

    pub fn run(&mut self) -> Result<[u32; vdpp2::COMMON_WORDS], String> {
        self.src
            .sync_write_end()
            .map_err(|error| format!("VDPP2 source flush: {error}"))?;
        self.histogram
            .sync_write_begin()
            .map_err(|error| format!("VDPP2 histogram clear begin: {error}"))?;
        self.histogram.as_mut_slice().fill(0);
        self.histogram
            .sync_write_end()
            .map_err(|error| format!("VDPP2 histogram clear flush: {error}"))?;

        let job = Vdpp2Job {
            src_fd: self.src.fd(),
            src_uv_offset: (self.config.src_width_vir * self.config.src_height_vir) as u32,
            dst_fd: self.dst.fd(),
            dst_uv_offset: (self.config.dst_width_vir * self.config.dst_height_vir) as u32,
            dst_c_fd: self.dst_c.as_ref().map_or(self.dst.fd(), DmaBuf::fd),
            hist_fd: self.histogram.fd(),
        };
        let registers = Vdpp2Registers::build(&self.config, &job)?;
        let offsets = [
            RegOffsetInfo {
                reg_idx: 25,
                offset: job.src_uv_offset,
            },
            RegOffsetInfo {
                reg_idx: 27,
                offset: if self.config.yuv_out_diff {
                    0
                } else {
                    job.dst_uv_offset
                },
            },
        ];
        let mut status = [0u32; vdpp2::COMMON_WORDS];
        let mut batch = Batch::new();
        batch.reg_write(&registers.yrgb_hor_coe, vdpp2::REG_OFF_YRGB_HOR_COE);
        batch.reg_write(&registers.yrgb_ver_coe, vdpp2::REG_OFF_YRGB_VER_COE);
        batch.reg_write(&registers.cbcr_hor_coe, vdpp2::REG_OFF_CBCR_HOR_COE);
        batch.reg_write(&registers.cbcr_ver_coe, vdpp2::REG_OFF_CBCR_VER_COE);
        batch.reg_write(&registers.zme_common, vdpp2::REG_OFF_ZME_COMMON);
        batch.reg_write(&registers.dmsr, vdpp2::REG_OFF_DMSR);
        batch.reg_write(&registers.hist, vdpp2::REG_OFF_DCI);
        batch.reg_write(&registers.es, vdpp2::REG_OFF_ES);
        batch.reg_write(&registers.sharp, vdpp2::REG_OFF_SHARP);
        batch.reg_offsets(&offsets);
        batch.reg_write(&registers.common, 0);
        batch.reg_read(&mut status, 0);
        self.service
            .submit_batch_plain_recovering(&mut batch)
            .map_err(|error| format!("VDPP2 submit: {error}"))?;
        self.service
            .poll_hw_finish_recovering()
            .map_err(|error| format!("VDPP2 poll: {error}"))?;
        vdpp2::check_completion_status(status[10])?;

        self.dst
            .sync_read_begin()
            .map_err(|error| format!("VDPP2 destination invalidate: {error}"))?;
        if let Some(dst_c) = &self.dst_c {
            dst_c
                .sync_read_begin()
                .map_err(|error| format!("VDPP2 chroma destination invalidate: {error}"))?;
        }
        self.histogram
            .sync_read_begin()
            .map_err(|error| format!("VDPP2 histogram invalidate: {error}"))?;
        Ok(status)
    }
}

fn append_rows(
    out: &mut Vec<u8>,
    src: &[u8],
    offset: usize,
    stride: usize,
    row_bytes: usize,
    rows: usize,
) -> Result<(), String> {
    for row in 0..rows {
        let start = offset
            .checked_add(
                row.checked_mul(stride)
                    .ok_or_else(|| "VDPP2 plane row offset overflow".to_string())?,
            )
            .ok_or_else(|| "VDPP2 plane row offset overflow".to_string())?;
        let end = start
            .checked_add(row_bytes)
            .ok_or_else(|| "VDPP2 plane row end overflow".to_string())?;
        let line = src
            .get(start..end)
            .ok_or_else(|| "VDPP2 destination plane is truncated".to_string())?;
        out.extend_from_slice(line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_number_is_29() {
        assert_eq!(VDPP_CLIENT, 29);
    }

    #[test]
    fn histogram_buffer_matches_the_c_api_size() {
        assert_eq!(HISTOGRAM_BYTES, 10_240);
    }
}
