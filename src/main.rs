#![allow(non_snake_case)]
#![allow(unused_imports)]
// use cpal::{Data, Sample, SampleFormat};
//
// fn main() {
//     let host = cpal::default_host();
//     let device = host
//         .default_output_device()
//         .expect("no output device available");
//
//     use cpal::traits::{DeviceTrait, HostTrait};
//     let mut supported_configs_range = device
//         .supported_output_configs()
//         .expect("error while querying configs");
//     let supported_config = supported_configs_range
//         .next()
//         .expect("no supported config?!")
//         .with_max_sample_rate();
//
//     let err_fn = |err| eprintln!("an error occurred on the output audio stream: {}", err);
//     let sample_format = supported_config.sample_format();
//     let config = supported_config.into();
//     let stream = match sample_format {
//         SampleFormat::F32 => device.build_output_stream(&config, write_silence::<f32>, err_fn),
//         SampleFormat::I16 => device.build_output_stream(&config, write_silence::<i16>, err_fn),
//         SampleFormat::U16 => device.build_output_stream(&config, write_silence::<u16>, err_fn),
//     }
//     .unwrap();
//
//     println!("Hello, world!");
// }
//
// fn write_silence<T: Sample>(data: &mut [T], _: &cpal::OutputCallbackInfo) {
//     for sample in data.iter_mut() {
//         *sample = Sample::from(&0.0);
//     }
// }

// use crossbeam::channel::bounded;
use ffmpeg_next::codec::context::Context as CodecContext;
use ffmpeg_next::codec::decoder;
use ffmpeg_next::format;
use ffmpeg_next::format::sample::Type::{Packed, Planar};
use ffmpeg_next::format::stream::Stream;
use ffmpeg_next::format::Sample as SampleType;
use ffmpeg_next::packet::Ref;
use ffmpeg_next::software::resampler;
use ffmpeg_next::util::channel_layout::ChannelLayout;
use ffmpeg_next::util::error::Error;
use ffmpeg_next::util::frame;
use std::io;
use std::vec::Vec;

fn main() -> io::Result<()> {
    let filename = String::from("res/An Ordinary Day-My Mister.mp3");
    // let filename = String::from("res/loop05.mp3");
    // let filename = String::from("res/loop05.wav");

    // init ffmpeg
    ffmpeg_next::init().unwrap();

    // set up an AVFormatContext
    let mut input_ctx = format::input(&filename).unwrap();

    // print information about the file format
    let input = input_ctx.format();
    println!("file format name: {}", input.name());
    println!("file format info: {}", input.description());

    // set up an AVCodecContext
    let mut codec_ctx = CodecContext::new();

    // get Audio Stream
    let stream: Stream;
    unsafe {
        let ptr = input_ctx.as_ptr();
        println!("number of streams: {}", (*ptr).nb_streams);
        stream = Stream::wrap(&input_ctx, 0);
    }
    // copy parameters from AVStream to AVCodecContext
    codec_ctx.set_parameters(stream.parameters()).unwrap();

    // open the decoder
    let mut decoder = codec_ctx.decoder().audio().unwrap();

    println!("[codec] samplerate: {}", decoder.rate());
    println!("[codec] number of channels: {}", decoder.channels());
    println!("[codec] channel layout: {:?}", decoder.channel_layout());
    println!("[codec] name: {}", decoder.codec().unwrap().name());
    println!("[codec] info: {}", decoder.codec().unwrap().description());

    let preferd_sample_format = SampleType::F32(Planar);
    let preferd_channle_layout = ChannelLayout::STEREO;
    decoder.request_format(preferd_sample_format);
    // decoder.set_channel_layout(ChannelLayout::STEREO);
    let mut _resampler_ctx = decoder
        .resampler(preferd_sample_format, ChannelLayout::STEREO, 48000)
        .unwrap();

    // write decoded (raw) audio data into buffer
    let mut output1: Vec<f32> = Vec::new();
    let mut output2: Vec<f32> = Vec::new();
    let mut _bof = frame::audio::Audio::empty();
    let mut of = frame::audio::Audio::empty();
    let mut raw_data = input_ctx.packets();
    while let Some(packet) = raw_data.next() {
        decoder.send_packet(&packet.1).unwrap();
        decode(&mut decoder, &mut of, &mut output1, &mut output2);
        /* read all the output frames (in general there may be any number of them */
    }
    decoder.send_eof().unwrap();
    decode(&mut decoder, &mut of, &mut output1, &mut output2);
    println!("{}", output1.len());
    println!("{}", output2.len());

    let mut i = 0;

    // 1. open a client
    let (client, _status) =
        jack::Client::new("rust_jack_sine", jack::ClientOptions::NO_START_SERVER).unwrap();

    // 2. register port
    let mut out1_port = client
        .register_port("sine_out1", jack::AudioOut::default())
        .unwrap();
    let mut out2_port = client
        .register_port("sine_out2", jack::AudioOut::default())
        .unwrap();

    // 3. define process callback handler
    let sample_rate = client.sample_rate();
    println!("{}", sample_rate);
    let process = jack::ClosureProcessHandler::new(
        move |_: &jack::Client, ps: &jack::ProcessScope| -> jack::Control {
            // Get output buffer
            let out1 = out1_port.as_mut_slice(ps);
            let out2 = out2_port.as_mut_slice(ps);
            let stereo_buffer = out1.iter_mut().zip(out2.iter_mut());

            // DSP
            for (l, r) in stereo_buffer {
                *l = output1[i];
                *r = output2[i];
                i += 1;
            }
            // Continue as normal
            jack::Control::Continue
        },
    );

    // 4. activate the client
    let _active_client = client.activate_async((), process).unwrap();

    // event loop
    let mut buffer = String::new();
    let stdin = io::stdin();
    stdin.read_line(&mut buffer)?;

    // 6. Optional deactivate. Not required since active_client will deactivate on
    // drop, though explicit deactivate may help you identify errors in
    // deactivate.
    _active_client.deactivate().unwrap();

    println!("{}", i);
    Ok(())
}

fn decode(
    decoder: &mut decoder::audio::Audio,
    of: &mut frame::audio::Audio,
    output1: &mut Vec<f32>,
    output2: &mut Vec<f32>,
) {
    loop {
        let ret = decoder.receive_frame(of);

        match ret {
            Err(Error::Other { errno: _EAGAIN }) => {
                // println!("EAGAIN");
                break;
            }
            Err(Error::Eof) => {
                // println!("Eof");
                break;
            }
            Ok(()) => {
                let left = of.data(0).chunks(4);
                let right = of.data(1).chunks(4);
                // let stereo = left.zip(right);
                let mut array: [u8; 4];
                let mut f: f32;

                for l in left {
                    array = [l[0], l[1], l[2], l[3]];
                    f = f32::from_le_bytes(array);
                    output1.push(f);
                }
                for r in right {
                    array = [r[0], r[1], r[2], r[3]];
                    f = f32::from_le_bytes(array);
                    output2.push(f);
                }
            }
            _ => panic!("error"),
        }
    }
}
