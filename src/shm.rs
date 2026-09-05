//! Shared-memory buffer helper built on sctk's `RawPool`.
//!
//! Each surface owns one pool, sliced into `count` equally sized buffers
//! (double buffering: we write the buffer we are about to attach while the
//! compositor still reads the previous one).

use smithay_client_toolkit::shm::{raw::RawPool, CreatePoolError, Shm};
use wayland_client::protocol::{wl_buffer, wl_shm};
use wayland_client::{Dispatch, Proxy, QueueHandle};

pub const PIXEL: usize = 4; // ARGB8888 / XRGB8888 bytes per pixel

pub struct ShmBuf {
    pub pool: RawPool,
    pub width: u32,
    pub height: u32,
    pub stride: i32,
    pub buffers: Vec<wl_buffer::WlBuffer>,
    pub current: usize,
    pub buf_size: usize,
    /// Whether the compositor has released each buffer (safe to rewrite).
    pub released: Vec<bool>,
}

impl ShmBuf {
    pub fn new<D>(
        shm: &Shm,
        qh: &QueueHandle<D>,
        width: u32,
        height: u32,
        count: usize,
        format: wl_shm::Format,
    ) -> Result<ShmBuf, CreatePoolError>
    where
        D: Dispatch<wl_buffer::WlBuffer, ()> + 'static,
    {
        let stride = (width as i32) * PIXEL as i32;
        let buf_size = stride as usize * height as usize;
        let mut pool = RawPool::new(buf_size * count, shm)?;
        let mut buffers = Vec::with_capacity(count);
        for i in 0..count {
            let offset = (i * buf_size) as i32;
            let b = pool.create_buffer(
                offset,
                width as i32,
                height as i32,
                stride,
                format,
                (),
                qh,
            );
            buffers.push(b);
        }
        let released = vec![true; count];
        Ok(ShmBuf { pool, width, height, stride, buffers, current: 0, buf_size, released })
    }

    /// Mutable view of the memory for buffer `idx`.
    pub fn canvas(&mut self, idx: usize) -> &mut [u8] {
        let base = idx * self.buf_size;
        &mut self.pool.mmap()[base..base + self.buf_size]
    }

    /// Mark the matching buffer as released by the compositor.
    pub fn mark_released(&mut self, buf: &wl_buffer::WlBuffer) {
        for (i, b) in self.buffers.iter().enumerate() {
            if b.id() == buf.id() {
                self.released[i] = true;
                return;
            }
        }
    }

    /// Advance to the next buffer only if the compositor has released it.
    /// Returns `(buffer, canvas)` to write into, or `None` if the next slot is
    /// still in use (caller should retry on the next tick).
    pub fn next_writeable(&mut self) -> Option<(wl_buffer::WlBuffer, &mut [u8])> {
        let n = self.buffers.len();
        let next = (self.current + 1) % n;
        if !self.released[next] {
            return None;
        }
        self.current = next;
        self.released[next] = false;
        Some((self.buffers[next].clone(), self.canvas(next)))
    }
}