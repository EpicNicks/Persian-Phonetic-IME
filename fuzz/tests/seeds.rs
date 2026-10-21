//! Runs the fuzz invariants over `fuzz/seeds/**` plus a deterministic pseudo-random
//! sweep; works without libFuzzer (`--no-default-features`).

use std::fs;
use std::path::Path;

fn seeds(dir: &str) -> Vec<Vec<u8>> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("seeds")
        .join(dir);
    let mut v: Vec<_> = fs::read_dir(p)
        .unwrap()
        .map(|e| fs::read(e.unwrap().path()).unwrap())
        .collect();
    v.push(Vec::new());
    v
}

fn random_inputs() -> impl Iterator<Item = Vec<u8>> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..2000).map(move |_| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let len = (x % 120) as usize;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    })
}

#[test]
fn session_seeds() {
    for s in seeds("session_keys").into_iter().chain(random_inputs()) {
        pime_fuzz::check_session(&s);
    }
}

#[test]
fn session_complete_seeds() {
    // Seeds shared with session_keys; byte 0 also selects the autocomplete settings.
    for s in seeds("session_keys").into_iter().chain(random_inputs()) {
        pime_fuzz::check_session_complete(&s);
    }
    // Hand-built: completions on (byte 0 = 0), type letters then exercise Down/Enter/Esc/digits.
    let nav = |i: u8| [3u8, 0x80 | (i << 1)];
    let mut seen = (0, 0);
    for vocab in [0u8, 1, 2] {
        for tail in 0..12u8 {
            let mut s = vec![0u8, vocab];
            for (i, letter) in [b's', b'l', b'a', b'm', b'k', b't'].iter().enumerate() {
                s.extend([0u8, (letter - b'a') << 3]);
                if i % 2 == 1 {
                    s.extend(nav(tail));
                }
            }
            s.extend(nav((tail + 1) % 12));
            s.extend(nav(2));
            let (c, l) = pime_fuzz::session_complete_coverage(&s);
            seen = (seen.0 + c, seen.1 + l);
        }
    }
    assert!(
        seen.0 > 0 && seen.1 > 0,
        "seeds never reach completions/learning: {seen:?}"
    );
}

#[test]
fn dict_seeds() {
    let valid = pime_fuzz::sample_dict();
    pime_fuzz::check_dict_bytes(&valid);
    for s in random_inputs() {
        pime_fuzz::check_dict_bytes(&s);
        let mut mutated = vec![1u8];
        mutated.extend(&s);
        pime_fuzz::check_dict_bytes(&mutated);
        mutated[0] = 3;
        pime_fuzz::check_dict_bytes(&mutated);
        // Corrupt the valid file directly: every single-byte flip must be rejected or safe.
        let mut v = valid.clone();
        let at = s.first().map_or(0, |&b| b as usize * 97 % v.len());
        v[at] ^= s.get(1).copied().unwrap_or(0xFF) | 1;
        pime_fuzz::check_dict_bytes(&v);
    }
}

#[test]
fn dict_tsv_seeds() {
    let tsvs: [&[u8]; 6] = [
        "سلام\t5\nسلامت\t4.2\nكتاب\t3\n".as_bytes(),
        "می\u{200C}خواهم\t4\nمیخواهم\t5\n\u{200C}\u{200C}ب\u{200C}\t2\n".as_bytes(),
        "# c\n\nx\t1\nسلام\t9\nسلام\t9\nسلام\tnan\n".as_bytes(),
        "مكة\t3\nعلي\t3\nرحمٰن\t2\nکـتاب\t2\n".as_bytes(),
        b"\xff\xfe\t1\n",
        b"",
    ];
    for t in tsvs {
        pime_fuzz::check_dict_tsv(t);
    }
    for s in random_inputs() {
        pime_fuzz::check_dict_tsv(&s);
    }
}

#[test]
fn history_seeds() {
    let valid = pime_fuzz::sample_history();
    pime_fuzz::check_history_bytes(&valid);
    for s in random_inputs() {
        pime_fuzz::check_history_bytes(&s);
        let mut v = valid.clone();
        for w in s.chunks(2) {
            let at = w[0] as usize * 131 % v.len();
            v[at] = w.get(1).copied().unwrap_or(0);
        }
        pime_fuzz::check_history_bytes(&v);
    }
}

#[test]
fn config_seeds() {
    for s in seeds("config_parse").into_iter().chain(random_inputs()) {
        pime_fuzz::check_config(&s);
    }
}
