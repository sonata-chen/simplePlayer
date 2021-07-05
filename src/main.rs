use cpal::traits::StreamTrait;
use std::io;

mod cpal_stream;
mod player;
use crate::cpal_stream::build_cpal_stream;
use crate::player::create_a_player;

fn main() {
    // let file_name = cstr!("loop05.wav");
    let file_name = "loop05.wav";
    let (mut c, mut p) = create_a_player();

    let s = build_cpal_stream(move |mut context| {
        p.render_next_block(&mut context);
    });
    s.play().unwrap();

    c.open(file_name.to_string());
    c.play();

    // event loop
    let stdin = io::stdin();
    let mut flag = true;
    loop {
        let mut buffer = String::new();
        stdin.read_line(&mut buffer).unwrap();
        if buffer == "p\n" {
            if flag == true {
                c.pause();
            } else {
                c.play();
            }
            flag = !flag;
        }
    }
}
