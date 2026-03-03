use crate::ratings::analyze::AnalyzedData;

pub fn visualize(data: AnalyzedData) {
    let mut vec = data
        .songs
        .iter()
        .map(|(song, analyzed)| (&song.name, analyzed.canonical_rating))
        .collect::<Vec<_>>();
    vec.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap());

    let n = 100000;
    println!("Top {n} Songs by Rating:");
    for (name, rating) in vec.iter().take(n) {
        println!("{name} - {rating}")
    }
}
