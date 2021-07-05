use anyhow::{anyhow, Context};
use rsmpeg::{
    self,
    avcodec::{AVCodec, AVCodecContext},
    avformat::AVFormatContextInput,
    avutil::{av_get_channel_layout_nb_channels, get_bytes_per_sample, is_planar, AVSamples},
    error::RsmpegError,
    ffi,
    swresample::SwrContext,
};
use std::ffi::CStr;
use std::slice::from_raw_parts_mut;

use ringbuf::{Consumer, Producer, RingBuffer};

use crate::cpal_stream::PlaybackContext;

enum State {
    Playing,
    Stopped,
}
enum Command {
    Open(String),
    Play,
    Pause,
}
pub fn create_a_player() -> (Controller, Player) {
    let ringbuf = RingBuffer::new(20 as usize);
    let (tx, rx) = ringbuf.split();
    (
        Controller { tx },
        Player {
            file_name: String::new(),
            decoder: None,
            sample_rate: 0,
            state: State::Stopped,
            rx,
        },
    )
}

pub struct Controller {
    tx: Producer<Command>,
}
impl Controller {
    pub fn open(&mut self, file_name: String) {
        self.tx.push(Command::Open(file_name));
    }
    pub fn play(&mut self) {
        self.tx.push(Command::Play);
    }
    pub fn pause(&mut self) {
        self.tx.push(Command::Pause);
    }
}
pub struct Player {
    file_name: String,
    decoder: Option<Decoder>,
    sample_rate: u32,
    state: State,
    rx: Consumer<Command>,
}

impl Player {
    pub fn open(&mut self, file_name: &str) {
        self.file_name = String::from(file_name);

        // Construct a C-style string
        let mut temp = String::from(file_name);
        temp.push('\0');

        self.decoder = Some(Decoder::open(
            CStr::from_bytes_with_nul(temp.as_bytes()).unwrap(),
        ));
    }
    pub fn render_next_block(&mut self, context: &mut PlaybackContext) {
        match self.rx.pop() {
            Some(Command::Open(f)) => {
                self.open(&f);
                if let Some(ref mut d) = self.decoder {
                    d.sample_rate(context.sample_rate);
                }
            }
            Some(Command::Play) => {
                if self.decoder.is_some() {
                    self.state = State::Playing;
                }
            }
            Some(Command::Pause) => {
                self.state = State::Stopped;
            }
            None => {}
        }

        if let State::Stopped = self.state {
            return;
        }

        if let Some(d) = self.decoder.as_mut() {
            let len = d.decode_to_buffer(context.output_buffer, context.buffer_len);
            if len == 0 {
                self.state = State::Stopped;
            }
        }
    }
}

struct Decoder {
    file_name: String,
    input_format_context: Option<AVFormatContextInput>,
    codec_context: Option<AVCodecContext>,
    resampler: Option<SwrContext>,
    samples: Option<AVSamples>,
    index_of_stream: Option<usize>,
    buffer: [f32; 4096],
    read_index: usize,
    write_index: usize,
}
unsafe impl Send for Decoder {}

impl Decoder {
    fn open(file_name: &CStr) -> Self {
        let mut input_format_context =
            AVFormatContextInput::open(file_name).expect("Failed to create a AVFormatContextInput");
        input_format_context.dump(0, file_name).unwrap();

        let mut decode_context: Option<AVCodecContext> = None;
        let mut index_of_stream: Option<usize> = None;

        for (index, input_stream) in input_format_context.streams().into_iter().enumerate() {
            let codecpar = input_stream.codecpar();
            let codec_type = codecpar.codec_type;

            decode_context = match codec_type {
                ffi::AVMediaType_AVMEDIA_TYPE_AUDIO => {
                    let codec_id = codecpar.codec_id;
                    let decoder = AVCodec::find_decoder(codec_id)
                        .with_context(|| anyhow!("audio decoder ({}) not found.", codec_id))
                        .unwrap();
                    let mut decode_context = AVCodecContext::new(&decoder);
                    decode_context.apply_codecpar(codecpar).unwrap();
                    decode_context.open(None).unwrap();
                    Some(decode_context)
                }
                _ => None,
            };
            if decode_context.is_some() {
                index_of_stream = Some(index);
                break;
            }
        }
        Self {
            file_name: file_name.to_string_lossy().into_owned(),
            input_format_context: Some(input_format_context),
            codec_context: decode_context,
            resampler: None,
            samples: None,
            index_of_stream,
            buffer: [0.0f32; 4096],
            read_index: 0,
            write_index: 0,
        }
    }

    fn sample_rate(&mut self, sample_rate: u32) {
        if let Some(d) = self.codec_context.as_ref() {
            self.resampler = SwrContext::new(
                ffi::AV_CH_LAYOUT_STEREO.into(),
                ffi::AVSampleFormat_AV_SAMPLE_FMT_FLT,
                sample_rate as i32,
                if d.channel_layout == 0 {
                    (unsafe { ffi::av_get_default_channel_layout(d.channels) }) as u64
                } else {
                    d.channel_layout
                },
                d.sample_fmt,
                d.sample_rate,
            );

            if let Some(ref mut r) = self.resampler {
                r.init().unwrap();
            }

            self.samples = AVSamples::new(
                av_get_channel_layout_nb_channels(ffi::AV_CH_LAYOUT_STEREO.into()),
                4096,
                ffi::AVSampleFormat_AV_SAMPLE_FMT_FLT,
                0,
            );
        }
    }

    fn decode_to_buffer(&mut self, output: &mut [f32], length: usize) -> usize {
        let mut i = 0 as usize;
        if let (Some(d), Some(f), Some(r), Some(s)) = (
            &mut self.codec_context,
            &mut self.input_format_context,
            &mut self.resampler,
            &mut self.samples,
        ) {
            let bytes_per_sample = get_bytes_per_sample(d.sample_fmt).unwrap();
            let nb_channels = d.channels;
            while i < length && self.read_index != self.write_index {
                output[i] = self.buffer[self.read_index];
                i += 1;
                self.read_index += 1;
                if self.read_index >= 4096 {
                    self.read_index = self.read_index - 4096;
                }
            }

            while i < length {
                let packet = f.read_packet().unwrap();
                match packet {
                    Some(_) => d.send_packet(packet.as_ref()).unwrap(),
                    None => {
                        let ret = d.send_packet(None);
                        match ret {
                            Ok(()) => {}
                            Err(RsmpegError::DecoderFlushedError) => break,
                            _ => panic!("{:?}", ret),
                        }
                    }
                };

                loop {
                    let frame = d.receive_frame();
                    match frame {
                        Ok(mut f) => {
                            let ptr = f.data_mut().as_ptr() as *const *const u8;

                            let buffer_size_per_channel = if is_planar(f.format) {
                                f.linesize_mut()[0] / bytes_per_sample
                            } else {
                                f.linesize_mut()[0] / nb_channels / bytes_per_sample
                            };
                            let ret = unsafe { r.convert(s, ptr, buffer_size_per_channel) };
                            match ret {
                                Ok(n) => {
                                    let n = (n as usize) * 2 * 4;
                                    let data = unsafe { from_raw_parts_mut(s.audio_data[0], n) };
                                    for d in data.chunks(4) {
                                        let sample = f32::from_le_bytes([d[0], d[1], d[2], d[3]]);
                                        if i < length {
                                            output[i] = sample;
                                            i += 1;
                                        } else {
                                            self.buffer[self.write_index] = sample;
                                            self.write_index += 1;
                                            if self.write_index >= 4096 {
                                                self.write_index = self.write_index - 4096;
                                            }
                                        }
                                    }
                                }
                                _ => panic!("convert"),
                            }
                        }
                        Err(RsmpegError::DecoderDrainError) => break,
                        Err(RsmpegError::DecoderFlushedError) => break,
                        Err(_) => panic!("error"),
                    };
                }
            }
        }
        i
    }
}

#[test]
fn decode() {
    use cpal::traits::DeviceTrait;
    use cpal::traits::HostTrait;
    use cpal::traits::StreamTrait;
    use cstr::cstr;
    // let file_name = cstr!("test.mp3");
    let file_name = cstr!("loop05.wav");
    let mut d = Decoder::open(file_name);
    d.sample_rate(44100);
    let mut output: Vec<f32> = Vec::new();
    let mut i = 0;
    loop {
        output.extend_from_slice(&[0.0; 2000]);
        println!("{:?}", output.len());
        let len = d.decode_to_buffer(&mut output[i..], 2000);
        println!("{}", len);
        i += len;
        if len == 0 {
            break;
        }
    }

    let mut i = 0;
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

        for d in data {
            *d = output[i];
            i += 1;
        }
    };

    let stream = output_device
        .build_output_stream(&config, callback, |err| eprintln!("{}", err))
        .expect("failed to open stream");
    stream.play().unwrap();

    let mut buffer = String::new();
    let stdin = std::io::stdin();
    stdin.read_line(&mut buffer).unwrap();
}
