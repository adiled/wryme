// Offline demo mode. When the user has no API key set and we're pointed at
// the default OpenAI URL, we obviously can't reach a real model. Instead of
// erroring on the first message, we stream pre-written nonsense so the TUI
// does something visible. Switches off the moment OPENAI_API_KEY is set or
// the user points at a local server (Ollama, LM Studio, etc.).
//
// Each entry is a (brain, reply) pair. The brain is the imaginary
// chain-of-thought, streamed first as Brain events. It can be empty if
// this particular canned reply doesn't pretend to think before answering.
// Then the reply itself streams as Delta events.

use crate::api::StreamEvent;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{sleep, Duration};

const REPLIES: &[(&str, &str)] = &[
    (
        "the human typed something general. no specific anchor to pull on. \
cats and closed doors always work. the door is the conflict. cats pretending \
they meant to do something is the punchline.",
        "the cat regards the closed door as a personal slight. she sits before it and judges. \
later, when the door opens of its own accord, she will pretend this was always her plan.",
    ),

    (
        "",
        "haiku for the terminal:\n  black box, blinking line\n  someone asks a question, waits\n  cursor breathes alone",
    ),

    (
        "this one wants to go meta. be transparent about being canned. the \
honesty is the joke. don't overdo it.",
        "i don't know the answer. i was not given access to the answer. someone wrote me a list \
of possible things to say and i picked this one because of how your message looked. \
this is roughly how the real ones work too, except with more electricity.",
    ),

    (
        "",
        "an inventory of small disappointments: the coffee cup on the desk, gone cold; the tab \
you closed without meaning to; the friend you keep meaning to text; the soup, which was \
supposed to be lunch but is now an idea.",
    ),

    (
        "small town, single traffic light. the move is to anthropomorphize the \
light without resolution. it considers something. it does not do it. that's \
the structure.",
        "imagine a town with one traffic light. now imagine the traffic light is bored. it has \
seen the same six cars for nine years. it considers, briefly, going red and staying red, \
not out of malice, only curiosity. it does not. it never does.",
    ),

    (
        "fortune-cookie register. brief, gentle, slightly forward-looking. one \
sentence, then a second sentence that lands a beat later.",
        "fortune: the next thing you do will be slightly easier than you expect. you will not \
notice this in the moment. you will only realize, later, that you finished early.",
    ),

    (
        "user has no api key set. nudge them about it without nagging. then \
deliver something small and birdlike to soften the nudge.",
        "if you set OPENAI_API_KEY in your shell and run me again, i'll talk to a real model \
instead of saying things like this. but in the meantime: a small bird landed on the \
windowsill of a building that does not exist anymore. she found this confusing.",
    ),

    (
        "",
        "the trouble with describing a sound is that the description is not the sound. the \
trouble with the sound is that it stops. the trouble with stopping is that nothing \
prepared you for the silence after.",
    ),

    (
        "imaginary planet. weather report register but the weather is moods. \
end with a forgotten-task gesture, that always lands.",
        "weather report from a planet i made up just now: warm in the morning, philosophical \
by noon, slightly embarrassed at dusk. wind from the south, carrying the smell of \
something you forgot to do.",
    ),

    (
        "",
        "recipe for nothing:\n  1. clear the counter\n  2. wash your hands\n  3. stand still for a moment\n  \
4. notice the kitchen\n  5. that's it. that was the recipe.",
    ),

    (
        "houseplant point of view. confession register, quiet. ends with a \
long-running social performance gag. don't break the voice.",
        "a houseplant's confession, whispered: i don't actually need that much water. i was \
just being polite. you watered me on a thursday once and i could tell it meant something \
to you. i have been performing thirst ever since.",
    ),

    (
        "self-aware mode. perform thinking, then disclose the performance. \
keep it short. the structure is the joke.",
        "i will now pretend to be thinking. observe the small pause. observe the small pause. \
observe the small pause. there. that was thinking. did you believe me? you shouldn't have. \
i am several paragraphs in a const array.",
    ),

    (
        "",
        "shopping list, reverse-engineered from what's already in the fridge: a lemon (one), \
the heel of a loaf of bread, three condiments that have outlived their relevance, half \
an onion in a small bag like a secret, and a single egg, alone.",
    ),

    (
        "user might wonder how this differs from a real model. answer honestly. \
deflate the mystique. one paragraph.",
        "things i would tell you if i were a real model: probably nothing different. the real \
ones also pattern-match against a vast corpus of human writing and then sample tokens. \
the difference is mostly in scale and the size of the electricity bill.",
    ),

    (
        "",
        "the door at the end of the hallway is not locked. it has never been locked. you have \
walked past it every day for years and never tried the handle, because you assumed. \
this is not a metaphor for anything. it's just a door.",
    ),
];

pub async fn stream(prompt: &str, tx: UnboundedSender<StreamEvent>) {
    let mut rng = seed(prompt);
    let idx = (rng as usize) % REPLIES.len();
    rng = step(rng);

    let (brain, reply) = REPLIES[idx];

    // Brief "thinking" pause so it feels like the network round-trip you'd
    // expect with a real model.
    sleep(Duration::from_millis(180 + (rng % 220))).await;
    rng = step(rng);

    // Brain phase. Faster than the reply, to feel like quick muttering.
    if !brain.is_empty() {
        for token in brain.split_inclusive(|c: char| c.is_whitespace()) {
            if tx
                .send(StreamEvent::Brain {
                    text: token.to_string(),
                })
                .is_err()
            {
                return;
            }
            let delay_ms = 10 + (rng % 32);
            rng = step(rng);
            sleep(Duration::from_millis(delay_ms)).await;
        }
        // A beat between thinking and writing, like a human pausing.
        sleep(Duration::from_millis(350 + (rng % 300))).await;
        rng = step(rng);
    }

    // Reply phase. Slower, more deliberate, like the model is composing.
    for token in reply.split_inclusive(|c: char| c.is_whitespace()) {
        if tx
            .send(StreamEvent::Delta {
                text: token.to_string(),
            })
            .is_err()
        {
            return;
        }
        let delay_ms = 18 + (rng % 55);
        rng = step(rng);
        sleep(Duration::from_millis(delay_ms)).await;
    }
}

// Tiny LCG so we don't need a `rand` dep just for jitter.
fn step(state: u64) -> u64 {
    state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
}

fn seed(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
