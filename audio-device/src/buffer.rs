use crate::audio_connection::AudioConnection;

pub struct UnsharedPlaybackSource<P: PlaybackSource>(pub P);

impl<P: PlaybackSource> MaybeSharedPlaybackSource<P> for UnsharedPlaybackSource<P> {
    fn with(&mut self, cb: impl FnOnce(&mut P)) {
        cb(&mut self.0)
    }
}

pub trait MaybeSharedPlaybackSource<P: PlaybackSource> {
    fn with(&mut self, cb: impl FnOnce(&mut P));
}

pub trait PlaybackSource {
    fn next_samples(&mut self, count: usize, callback: impl FnOnce((&[f32], Option<&[f32]>)));
}

pub trait MicSink {
    fn append_sample_from_mic(&mut self, sample: f32);
    fn publish_chunks(&mut self);
}

pub trait InterruptReceiver<R> {
    fn registration(&self) -> &R;
    fn invoke<AC: AudioConnection>(&mut self, connection: &AC);
}

pub struct NoopInterruptReceiver;

impl<R> InterruptReceiver<R> for NoopInterruptReceiver {
    fn registration(&self) -> &R {
        unreachable!();
    }

    fn invoke<AC: AudioConnection>(&mut self, _: &AC) {}
}
