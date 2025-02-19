// ############################## TEST UTILITIES ##############################

use crate::prelude::*;

pub fn craft_random_text_of_len(char_length: usize) -> String {
    use rand::distr::{SampleString, StandardUniform};

    let random_string: String = StandardUniform.sample_string(&mut rand::rng(), char_length);
    //println!("random_valid_text: [[[{random_valid_text}]]]");

    random_string
}

pub fn craft_random_text() -> String {
    let random_len = rand::random_range(0..(isize::MAX as usize));
    let random_valid_text = craft_random_text_of_len(random_len);

    //println!("random_valid_text: [[[{random_valid_text}]]]");

    random_valid_text
}

pub fn craft_random_msg(username: &Username) -> Message {
    let random_msg_len = rand::random_range(0..Message::MAX_CONTENT_LEN);
    let valid_text = craft_random_text_of_len(random_msg_len);
    Message::new(username, &valid_text).expect("The random message construction failed.")
}

// ############################## TEST ##############################
