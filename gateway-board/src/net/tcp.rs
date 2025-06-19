use core::{
    alloc::{GlobalAlloc, Layout},
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use embassy_net::{tcp::TcpSocket, Stack};

const TCP_BUFFER_SIZE: usize = 1536;

/// Not quite safe abstraction for allocating a TCP socket's buffers in the heap.
/// The struct owns its buffers, memory is released upon dropping.
pub struct BoxedTcpSocket<'a> {
    buffers: *mut Buffers,
    sock: ManuallyDrop<TcpSocket<'a>>,
}

#[repr(C)]
struct Buffers {
    rx_buffer: [u8; TCP_BUFFER_SIZE],
    tx_buffer: [u8; TCP_BUFFER_SIZE],
}

const BUFFERS_LAYOUT: Layout = Layout::new::<Buffers>();

impl<'a> BoxedTcpSocket<'a> {
    pub fn new(stack: Stack<'a>) -> Result<Self, ()> {
        unsafe {
            // SAFETY: BUFFERS_LAYOUT has non-zero size
            let mut buffers: NonNull<Buffers> =
                match NonNull::new(esp_alloc::HEAP.alloc_zeroed(BUFFERS_LAYOUT).cast()) {
                    Some(buffers) => buffers,
                    None => return Err(()),
                };

            // SAFETY:
            // - pointer is non-null, aligned, can be dereferenced
            // - `Buffers` is fully initializd (to zero).
            // - this is the only active pointer to this memory
            let Buffers {
                rx_buffer,
                tx_buffer,
            } = buffers.as_mut();

            let sock = ManuallyDrop::new(TcpSocket::new(stack, rx_buffer, tx_buffer));
            Ok(BoxedTcpSocket {
                buffers: buffers.as_ptr(),
                sock,
            })
        }
    }
}

impl Drop for BoxedTcpSocket<'_> {
    fn drop(&mut self) {
        debug_assert!(!self.buffers.is_null());

        unsafe {
            // SAFETY: socket is fully initialized, buffers are not null
            ManuallyDrop::drop(&mut self.sock);
            let mut buffers: *mut Buffers = core::ptr::null_mut();
            core::mem::swap(&mut self.buffers, &mut buffers);
            esp_alloc::HEAP.dealloc(buffers.cast::<u8>(), BUFFERS_LAYOUT);
        }
    }
}

impl<'a> embedded_io_async::ErrorType for BoxedTcpSocket<'a> {
    type Error = <TcpSocket<'a> as embedded_io_async::ErrorType>::Error;
}

impl embedded_io_async::Read for BoxedTcpSocket<'_> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let res = self.sock.read(buf).await;
        #[cfg(feature = "tcp-debug")]
        match res {
            Ok(read) => defmt::trace!(
                "tcp: read() ({=usize} bytes): {=[u8]:a}",
                read,
                &buf[..read]
            ),
            Err(e) => defmt::trace!("tcp: read() error: {:?}", e),
        }
        res
    }

    async fn read_exact(
        &mut self,
        buf: &mut [u8],
    ) -> Result<(), embedded_io_async::ReadExactError<Self::Error>> {
        let res = self.sock.read_exact(buf).await;
        #[cfg(feature = "tcp-debug")]
        match res {
            Ok(()) => defmt::trace!(
                "tcp: read_exact() ({=usize} bytes): {=[u8]:a}",
                buf.len(),
                buf
            ),
            Err(e) => defmt::trace!("tcp: read_exact() error: {:?}", e),
        }
        res
    }
}

impl embedded_io_async::Write for BoxedTcpSocket<'_> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        #[cfg(feature = "tcp-debug")]
        defmt::trace!("tcp: write: {=[u8]:a}", buf);
        self.sock.write(buf).await
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        #[cfg(feature = "tcp-debug")]
        defmt::trace!("tcp: flush");
        self.sock.flush().await
    }

    async fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        #[cfg(feature = "tcp-debug")]
        defmt::trace!("tcp: write_all: {=[u8]:a}", buf);
        self.sock.write_all(buf).await
    }
}

impl<'a> Deref for BoxedTcpSocket<'a> {
    type Target = TcpSocket<'a>;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.sock
    }
}

impl DerefMut for BoxedTcpSocket<'_> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sock
    }
}
