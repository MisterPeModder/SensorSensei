use core::future::Future;

pub trait AsyncEncoder {
    type Error: core::error::Error;

    async fn emit_bytes(&mut self, buf: &[u8]) -> Result<(), Self::Error>;

    #[inline]
    async fn emit<T: AsyncEncode<Self>>(&mut self, value: T) -> Result<(), Self::Error> {
        value.encode(self).await
    }

    async fn emit_from_iter<T: AsyncEncode<Self>, I>(&mut self, iter: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = T>,
    {
        for value in iter {
            value.encode(self).await?;
        }
        Ok(())
    }
}

pub trait AsyncEncode<E: AsyncEncoder + ?Sized> {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error>;
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
    };
}

tuple_encoding!(T0);
tuple_encoding!(T0, T1);
tuple_encoding!(T0, T1, T2);
tuple_encoding!(T0, T1, T2, T3);

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

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for f32 {
    #[inline]
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        // encode f32 as 4 little endian bytes for now
        encoder.emit_bytes(&self.to_le_bytes()).await
    }
}
