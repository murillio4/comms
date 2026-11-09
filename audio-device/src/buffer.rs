use crate::audio_connection::AudioConnection;

pub struct UnsharedPlaybackSource<P: PlaybackSource>(pub P);

impl<P: PlaybackSource> MaybeSharedPlaybackSource<P> for UnsharedPlaybackSource<P> {
    fn with<T>(&mut self, cb: impl FnOnce(&mut P) -> T) -> T {
        cb(&mut self.0)
    }
}

pub trait MaybeSharedPlaybackSource<P: PlaybackSource> {
    fn with<T>(&mut self, cb: impl FnOnce(&mut P) -> T) -> T;
}

pub trait PlaybackSource {
    fn next_samples(&mut self, count: usize, callback: impl FnOnce((&[f32], Option<&[f32]>)));
    fn drop_samples(&mut self);
}

pub trait MicSink {
    fn append_sample_from_mic(&mut self, sample: f32);
    fn publish_chunks(&mut self);
}

pub trait InterruptReceiver<R> {
    fn registration(&self) -> &R;
    fn optional_registration(&self) -> Option<&R> {
        Some(self.registration())
    }
    fn invoke<AC: AudioConnection>(&mut self, connection: &AC);
}

pub struct NoopInterruptReceiver;

impl<R> InterruptReceiver<R> for NoopInterruptReceiver {
    fn optional_registration(&self) -> Option<&R> {
        None
    }

    fn registration(&self) -> &R {
        unreachable!();
    }

    fn invoke<AC: AudioConnection>(&mut self, _: &AC) {}
}
