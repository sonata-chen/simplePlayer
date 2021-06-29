use cpal::traits::{DeviceTrait, HostTrait};
use cpal::Stream;

pub struct PlaybackContext<'a> {
    pub sample_rate: u32,
    pub output_buffer: &'a mut [f32],
    pub buffer_len: usize,
    pub num_of_channel: u32,
}
pub fn build_cpal_stream(
    mut render_next_block: impl FnMut(&mut PlaybackContext) + Send + 'static,
) -> Stream {
    let host = cpal::default_host();
    let output_device = host.default_output_device().expect("no output found");
    let config = output_device
        .default_output_config()
        .expect("no default output config")
        .config();

    let sample_rate = config.sample_rate.0 as u32;
    println!("config: {}", sample_rate);
    let num_channels = config.channels as u32;
    println!("channels: {}", num_channels);

    let callback = move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        for sample in data.iter_mut() {
            *sample = 0.0;
        }

        let len = data.len();

        let mut context = PlaybackContext {
            sample_rate,
            output_buffer: data,
            buffer_len: len,
            num_of_channel: num_channels,
        };

        render_next_block(&mut context);
    };

    output_device
        .build_output_stream(&config, callback, |err| eprintln!("{}", err))
        .expect("failed to open stream")
}
