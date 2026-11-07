use rk_mpp_rs::hal::vdpu383_regs as reg;

#[test]
fn vdpu383_keeps_the_276_word_rkvdec_register_window() {
    assert_eq!(reg::N_WORDS, 276);
    let mut regs = reg::Regs::new();
    regs.set(reg::w8::REG8_DEC_MODE, 7)
        .set(reg::w168::REG168_DECOUT_BASE, 0x1234_5000)
        .set(reg::w169::REG169_ERROR_REF_BASE, 0x1235_6000);
    assert_eq!(regs.get(reg::w8::REG8_DEC_MODE), 7);
    assert_eq!(regs.get(reg::w168::REG168_DECOUT_BASE), 0x1234_5000);
    assert_eq!(regs.get(reg::w169::REG169_ERROR_REF_BASE), 0x1235_6000);
}

#[test]
fn vdpu383_control_fields_match_the_rk3576_header() {
    assert_eq!(
        (
            reg::w9::BUF_EMPTY_EN.word,
            reg::w9::BUF_EMPTY_EN.lsb,
            reg::w9::BUF_EMPTY_EN.width
        ),
        (9, 4, 1)
    );
    assert_eq!(
        (reg::w9::PIX_RANGE_DET_E.word, reg::w9::PIX_RANGE_DET_E.lsb),
        (9, 7)
    );
    assert_eq!(
        (
            reg::w14::DEC_LINE_IRQ_STEP.word,
            reg::w14::DEC_LINE_IRQ_STEP.lsb,
            reg::w14::DEC_LINE_IRQ_STEP.width
        ),
        (14, 0, 16)
    );
}
