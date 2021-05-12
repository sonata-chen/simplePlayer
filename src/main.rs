#![allow(non_snake_case)]
#![allow(unused_imports)]

use ffmpeg::codec::Context as CodecContext;
use ffmpeg::decoder;
use ffmpeg::format;
use ffmpeg::format::context::input::PacketIter;
use ffmpeg::format::context::Input as InputContext;
use ffmpeg::format::stream::Stream;
use ffmpeg::frame;

use ffmpeg::format::sample::Type::{Packed, Planar};
use ffmpeg::format::Sample as SampleType;
use ffmpeg::ChannelLayout;

use ffmpeg::software::resampler;

use ffmpeg::Error;

use std::io;
use std::vec::Vec;

fn main() -> io::Result<()> {
    let file_name = String::from("res/chill.mp3");
    // let file_name = String::from("res/An Ordinary Day-My Mister.mp3");
    // let filename = String::from("res/loop05.mp3");
    // let filename = String::from("res/loop05.wav");

    // init ffmpeg
    ffmpeg::init().unwrap();

    // set up an AVFormatContext
    let mut audio_file = AudioFile::open(&file_name);

    // print information about the file format
    println!("file format name: {}", audio_file.format_name());
    println!("file format info: {}", audio_file.format_details());

    // set up an AVCodecContext
    // open the decoder
    let mut audio_decoder = AudioDecoder::new(&audio_file);
    audio_decoder.info();

    // write decoded (raw) audio data into buffer
    let buffer = audio_decoder.decode_to_buffer(&mut audio_file.input_ctx.packets());

    // let preferd_sample_format = SampleType::F32(Planar);
    // let preferd_channle_layout = ChannelLayout::STEREO;
    // decoder.request_format(preferd_sample_format);
    // // decoder.set_channel_layout(ChannelLayout::STEREO);
    // let mut _resampler_ctx = decoder
    //     .resampler(preferd_sample_format, ChannelLayout::STEREO, 48000)
    //     .unwrap();

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
                *l = buffer.0[i];
                *r = buffer.1[i];
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

struct AudioFile {
    input_ctx: InputContext,
}

impl AudioFile {
    fn open(file_name: &str) -> Self {
        Self {
            input_ctx: format::input(file_name).unwrap(),
        }
    }

    fn format_name(&self) -> String {
        String::from(self.input_ctx.format().name())
    }

    fn format_details(&self) -> String {
        String::from(self.input_ctx.format().description())
    }
}

struct AudioDecoder {
    decoder: decoder::Audio,
    of: frame::Audio,
}

impl AudioDecoder {
    fn new(audio_file: &AudioFile) -> Self {
        // get Audio Stream
        let stream: Stream;
        unsafe {
            let ptr = audio_file.input_ctx.as_ptr();
            println!("number of streams: {}", (*ptr).nb_streams);
            stream = Stream::wrap(&audio_file.input_ctx, 0);
        }

        Self {
            // open the decoder
            decoder: stream.codec().decoder().audio().unwrap(),
            // create an empty frame
            of: frame::Audio::empty(),
        }
    }

    fn info(&self) {
        let decoder = &self.decoder;
        println!("[codec] samplerate: {}", decoder.sample_rate());
        println!("[codec] number of channels: {}", decoder.channels());
        println!("[codec] channel layout: {:?}", decoder.channel_layout());
        println!("[codec] name: {}", decoder.codec().unwrap().name());
        println!("[codec] info: {}", decoder.codec().unwrap().description());
    }

    fn decode_to_buffer(&mut self, packets: &mut PacketIter) -> (Vec<f32>, Vec<f32>) {
        let mut output1: Vec<f32> = Vec::new();
        let mut output2: Vec<f32> = Vec::new();
        while let Some(Ok(packet)) = packets.next() {
            self.decoder.send_packet(&packet.1).unwrap();
            self.decode(&mut output1, &mut output2);
            /* read all the output frames (in general there may be any number of them */
        }
        self.decoder.send_eof().unwrap();
        self.decode(&mut output1, &mut output2);
        (output1, output2)
    }

    fn decode(&mut self, output1: &mut Vec<f32>, output2: &mut Vec<f32>) {
        loop {
            let ret = self.decoder.receive_frame(&mut (self.of));

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
                    let left = self.of.data(0).chunks(4);
                    let right = self.of.data(1).chunks(4);
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
}

