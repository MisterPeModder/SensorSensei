use core::future::Future;

pub trait AsyncEncoder {
    type Error: core::error::Error;

    async fn emit_bytes(&mut self, buf: &[u8]) -> Result<(), Self::Error>;

    #[inline]
    async fn emit<T: AsyncEncode<Self>>(&mut self, value: T) -> Result<(), Self::Error> {
        value.encode(self).await
    }
}

pub trait AsyncDecoder {
    type Error: core::error::Error;

    /// "Blocks" until the full buffer is filled.
    /// This is a no-op if `buf` is an empty slice.
    async fn read_bytes(&mut self, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Returns the number of bytes read from the first call to [`read_bytes`].  
    /// Successive calls to this function will yield a value that is always equal or greater than the previous call,
    /// except in the case of overflow.
    /// Note: this number is allowed to overflow (in case of *really* long-running programs).
    fn current_offset(&self) -> usize;

    fn decoding_error(&self) -> Self::Error;

    /// Reads a value of type `F` from the stream.  
    /// Returns the value and the number of bytes that were read.
    #[inline]
    async fn read<T: AsyncDecode<Self>>(&mut self) -> Result<T, Self::Error> {
        T::decode(self).await
    }

    /// Reads exactly `n` bytes from a stream, discarding them.
    async fn read_discard(&mut self, mut n: usize) -> Result<(), Self::Error> {
        let mut buf = [0u8; 16];

        while n > 0 {
            let to_discard = n.min(buf.len());
            self.read_bytes(&mut buf[..to_discard]).await?;
            n -= to_discard;
        }
        Ok(())
    }
}

pub trait AsyncEncode<E: AsyncEncoder + ?Sized> {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error>;
}

pub trait AsyncDecode<D: AsyncDecoder + ?Sized> {
    async fn decode(decoder: &mut D) -> Result<Self, D::Error>
    where
        Self: Sized;
}

// impl AsyncEncode for refs for convenience
impl<E, T> AsyncEncode<E> for &T
where
    E: AsyncEncoder + ?Sized,
    T: AsyncEncode<E> + Copy,
{
    #[inline(always)]
    fn encode(self, encoder: &mut E) -> impl Future<Output = Result<(), E::Error>> {
        (*self).encode(encoder)
    }
}

/// Generates impls for tuples (T0, ...)
macro_rules! tuple_encoding {
    ($($t:ident),+) => {
        impl<E, $($t,)+> AsyncEncode<E> for ($($t,)+)
        where
            E: AsyncEncoder + ?Sized,
            $($t : AsyncEncode<E>,)+
        {
            #[allow(non_snake_case)]
            async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
                let ($($t,)+) = self;
                $($t.encode(encoder).await?;)+
                Ok(())
            }
        }

        impl<D, $($t,)+> AsyncDecode<D> for ($($t,)+)
        where
            D: AsyncDecoder + ?Sized,
            $($t : AsyncDecode<D>,)+
        {
            #[allow(non_snake_case)]
            async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
                $(let $t = $t::decode(decoder).await?;)+
                Ok(($($t,)+))
            }
        }
    };
}

tuple_encoding!(T0, T1);
tuple_encoding!(T0, T1, T2);

pub trait ToLeb128Ext<const N: usize> {
    fn to_leb128(self, buf: &mut [u8; N]) -> &[u8];
}

macro_rules! uleb128_encoding {
    ($type:ty; $n:expr) => {
        /// Encodes this number using unsigned little endian base128 (ULEB128)
        impl ToLeb128Ext<$n> for $type {
            fn to_leb128(self, buf: &mut [u8; $n]) -> &[u8] {
                let mut val = self;
                let mut i = 0usize;
                while val > 0x7f as $type {
                    buf[i] = 0x80 | ((val & (0x7f as $type)) as u8);
                    val >>= 7;
                    i += 1;
                }
                buf[i] = val as u8;
                &buf[..i + 1]
            }
        }

        impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for $type {
            async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
                let mut buf = [0u8; $n];
                encoder.emit_bytes(self.to_leb128(&mut buf)).await
            }
        }

        impl<D: AsyncDecoder + ?Sized> AsyncDecode<D> for $type {
            async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
                let mut byte = [0u8; 1];
                let mut val: $type = 0;
                let mut shift: $type = 0;

                for _ in 0..$n {
                    decoder.read_bytes(&mut byte).await?;
                    val |= ((byte[0] & 0x7f) as $type) << shift;
                    shift += 7;
                    if byte[0] & 0x80 == 0 {
                        break;
                    }
                }

                Ok(val)
            }
        }
    };
}

macro_rules! sleb128_encoding {
    ($type:ty; $n:expr) => {
        /// Encodes this number using signed little endian base128 (ULEB128)
        impl ToLeb128Ext<$n> for $type {
            fn to_leb128(self, buf: &mut [u8; $n]) -> &[u8] {
                let mut val = self;
                let mut i = 0usize;
                loop {
                    let byte = (val & 0x7f) as u8;
                    val >>= 7;
                    if (val == 0 && byte & 0x40 == 0) || (val == -1 && byte & 0x40 != 0) {
                        buf[i] = byte;
                        break;
                    } else {
                        buf[i] = 0x80 | byte;
                        i += 1;
                    }
                }
                &buf[..i + 1]
            }
        }

        impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for $type {
            async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
                let mut buf = [0u8; $n];
                encoder.emit_bytes(self.to_leb128(&mut buf)).await
            }
        }

        impl<D: AsyncDecoder + ?Sized> AsyncDecode<D> for $type {
            async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
                let mut byte = [0u8; 1];
                let mut val: $type = 0;
                let mut shift: u32 = 0;

                for _ in 0..$n {
                    decoder.read_bytes(&mut byte).await?;

                    val |= ((byte[0] & 0x7f) as $type) << shift;
                    shift += 7;

                    if byte[0] & 0x80 == 0 {
                        if shift < 64 && (byte[0] & 0x40) != 0 {
                            val |= (!0 as $type) << shift;
                        }
                        break;
                    }
                }

                Ok(val)
            }
        }
    };
}

uleb128_encoding![u32; 5];
uleb128_encoding![u64; 10];
sleb128_encoding![i64; 10];

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for u8 {
    #[inline]
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        encoder.emit_bytes(&self.to_le_bytes()).await
    }
}

impl<D: AsyncDecoder + ?Sized> AsyncDecode<D> for u8 {
    #[inline]
    async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
        let mut buf = [0u8; 1];
        decoder.read_bytes(&mut buf).await?;
        Ok(buf[0])
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for f32 {
    #[inline]
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        // encode f32 as 4 little endian bytes for now
        encoder.emit_bytes(&self.to_le_bytes()).await
    }
}

impl<D: AsyncDecoder + ?Sized> AsyncDecode<D> for f32 {
    #[inline]
    async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
        let mut buf = [0u8; 4];
        decoder.read_bytes(&mut buf).await?;
        Ok(f32::from_le_bytes(buf))
    }
}
