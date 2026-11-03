use super::*;

#[test]
fn urlsplit_gives_the_scheme_netloc_path_query_and_fragment() {
    let cases: &[(&str, [&str; 5])] = &[
        ("/runs?cwd=%2Fp&token=t", ["", "", "/runs", "cwd=%2Fp&token=t", ""]),
        ("/runs", ["", "", "/runs", "", ""]),
        ("/runs?", ["", "", "/runs", "", ""]),
        ("/a#b?c", ["", "", "/a", "", "b?c"]),
        ("/a?b#c?d", ["", "", "/a", "b", "c?d"]),
        ("  /x?y", ["", "", "/x", "y", ""]),
        ("\t/r\nu\rns?a=1", ["", "", "/runs", "a=1", ""]),
        ("*", ["", "", "*", "", ""]),
        ("http://127.0.0.1:9/runs?cwd=x", ["http", "127.0.0.1:9", "/runs", "cwd=x", ""]),
        ("HTTP://h", ["http", "h", "", "", ""]),
        ("//h/p?q", ["", "h", "/p", "q", ""]),
        ("x:y", ["x", "", "y", "", ""]),
        ("1x:y", ["", "", "1x:y", "", ""]),
        ("a+b-c.d:rest", ["a+b-c.d", "", "rest", "", ""]),
        ("a_b:rest", ["", "", "a_b:rest", "", ""]),
        (":x", ["", "", ":x", "", ""]),
        ("/c:d", ["", "", "/c:d", "", ""]),
        ("mailto:", ["mailto", "", "", "", ""]),
        ("/p?q?r", ["", "", "/p", "q?r", ""]),
        ("", ["", "", "", "", ""]),
    ];
    for (url, parts) in cases {
        let got = urlsplit(url);
        assert_eq!([&*got.scheme, &*got.netloc, &*got.path, &*got.query, &*got.fragment], *parts, "{url:?}");
    }
}

#[test]
fn parse_qsl_gives_each_pair_unquoted_in_order() {
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("cwd=%2Fp&token=t", &[("cwd", "/p"), ("token", "t")]),
        ("a=1&a=2", &[("a", "1"), ("a", "2")]),
        ("a", &[("a", "")]),
        ("a=", &[("a", "")]),
        ("=v", &[("", "v")]),
        ("&&a=1&", &[("a", "1")]),
        ("a=b=c", &[("a", "b=c")]),
        ("a+b=c+d", &[("a b", "c d")]),
        ("x=%41%4a%4A", &[("x", "AJJ")]),
        ("x=%", &[("x", "%")]),
        ("x=%4", &[("x", "%4")]),
        ("x=%zz", &[("x", "%zz")]),
        ("x=%C3%A9", &[("x", "é")]),
        ("x=%ED%B2%80", &[("x", "���")]),
        ("x=%FF", &[("x", "�")]),
        ("x=%C3", &[("x", "�")]),
        ("x=%e2%82", &[("x", "�")]),
        ("é=ü%C3%A9", &[("é", "üé")]),
        ("a;b=c", &[("a;b", "c")]),
        ("", &[]),
        ("token=%20", &[("token", " ")]),
    ];
    for (query, pairs) in cases {
        let want: Vec<(String, String)> = pairs.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
        assert_eq!(parse_qsl(query), want, "{query:?}");
    }
}
