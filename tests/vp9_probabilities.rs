use rk_mpp_rs::codec::vp9::prob::{DEFAULT_COEF_PROBS, DEFAULT_HEADER_PROBS};

#[test]
fn default_header_probabilities_match_mpp() {
    assert_eq!(DEFAULT_HEADER_PROBS.y_mode[0][0], 65);
    assert_eq!(DEFAULT_HEADER_PROBS.y_mode[3][8], 29);
    assert_eq!(DEFAULT_HEADER_PROBS.uv_mode[9][8], 125);
    assert_eq!(DEFAULT_HEADER_PROBS.filter[0], [235, 162]);
    assert_eq!(DEFAULT_HEADER_PROBS.filter[3], [149, 144]);
    assert_eq!(DEFAULT_HEADER_PROBS.mv_comp[0].classes[0], 224);
    assert_eq!(DEFAULT_HEADER_PROBS.mv_comp[1].classes[0], 216);
    assert_eq!(DEFAULT_HEADER_PROBS.partition[0][0], [222, 34, 30]);
    assert_eq!(DEFAULT_HEADER_PROBS.partition[3][3], [121, 104, 114]);
}

#[test]
fn default_coefficient_probabilities_match_mpp() {
    assert_eq!(DEFAULT_COEF_PROBS[0][0][0][0][0], [195, 29, 183]);
    assert_eq!(DEFAULT_COEF_PROBS[0][1][1][5][5], [8, 23, 61]);
    assert_eq!(DEFAULT_COEF_PROBS[3][1][1][5][5], [1, 16, 6]);
}
