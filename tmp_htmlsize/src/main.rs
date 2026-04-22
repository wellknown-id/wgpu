use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{NodeData, RcDom};
fn main() {
    let dom = parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut b"<title>Test</title>".as_ref())
        .unwrap();
    println!("{:?}", dom.document);
}
