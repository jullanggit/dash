use crate::ratings::analyze::AnalyzedData;

pub fn visualize(data: AnalyzedData) {
    let mut vec = data
        .songs
        .iter()
        .map(|(song, analyzed)| (&song.name, analyzed.canonical_rating))
        .collect::<Vec<_>>();
    vec.sort_by_key(|(_, rating)| rating);

    println!("Top 10 Songs by Rating:");
    for (name, rating) in &vec[0..10] {
        println!("{name} - {rating}")
    }
}
