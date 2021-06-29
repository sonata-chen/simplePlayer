use anyhow::{anyhow, Context};
use rsmpeg::{
    self,
    avcodec::{AVCodec, AVCodecContext},
    avformat::AVFormatContextInput,
    swresample::SwrContext,
    error::RsmpegError,
    ffi,
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
            Some(Command::Open(f)) => self.open(&f),
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
            index_of_stream,
            buffer: [0.0f32; 4096],
            read_index: 0,
            write_index: 0,
        }
    }

    fn decode_to_buffer(&mut self, output: &mut [f32], length: usize) -> usize {
        let mut i = 0 as usize;
        if let (Some(d), Some(f)) = (
            self.codec_context.as_mut(),
            self.input_format_context.as_mut(),
        ) {
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
                            Err(RsmpegError::DecoderFlushedError) => {
                                println!("already flushed");
                                break;
                            }
                            _ => panic!("{:?}", ret),
                        }
                    }
                };

                loop {
                    let frame = d.receive_frame();
                    match frame {
                        Ok(mut f) => {
                            let data = unsafe {
                                from_raw_parts_mut(f.data_mut()[0], f.linesize_mut()[0] as usize)
                            }
                            .chunks(4);

                            for d in data {
                                let sample = f32::from_le_bytes([d[0], d[1], d[2], d[3]]);
                                if i < length {
                                    output[i] = sample;
                                } else {
                                    self.buffer[self.write_index] = sample;
                                    self.write_index += 1;
                                    if self.write_index >= 4096 {
                                        self.write_index = self.write_index - 4096;
                                    }
                                }
                                i += 1;
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
