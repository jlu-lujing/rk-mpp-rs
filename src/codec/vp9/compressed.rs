//! VP9 arithmetic-coded compressed header.

use super::parser::{FrameKind, Header, Vp9Error};
use super::prob::{CoefficientProbabilities, ProbabilityContext, ProbabilityUpdates};
use super::range::RangeDecoder;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxMode {
    Only4x4,
    Only8x8,
    Only16x16,
    Only32x32,
    Select,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompoundPredictionMode {
    SingleReference,
    CompoundReference,
    Select,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompressedHeader {
    pub tx_mode: TxMode,
    pub allow_compound_prediction: bool,
    pub compound_prediction_mode: CompoundPredictionMode,
    pub probabilities: ProbabilityContext,
    pub updates: ProbabilityUpdates,
}

pub fn parse(
    header: &Header,
    reference: &ProbabilityContext,
    data: &[u8],
) -> Result<CompressedHeader, Vp9Error> {
    let mut reader = RangeDecoder::new(data)?;
    if reader.read_bool(128)? {
        return Err(Vp9Error::BadCompressedHeaderMarker);
    }

    let mut probabilities = reference.clone();
    let mut updates = ProbabilityUpdates::default();
    let tx_mode = if header.quantization.lossless {
        TxMode::Only4x4
    } else {
        let value = reader.read_bits(2)? as u8;
        let value = if value == 3 {
            value + u8::from(reader.read_bool(128)?)
        } else {
            value
        };
        match value {
            0 => TxMode::Only4x4,
            1 => TxMode::Only8x8,
            2 => TxMode::Only16x16,
            3 => TxMode::Only32x32,
            4 => TxMode::Select,
            value => return Err(Vp9Error::UnsupportedTxMode(value)),
        }
    };

    if tx_mode == TxMode::Select {
        for i in 0..2 {
            update_probability(
                &mut reader,
                &mut probabilities.header.tx8p[i],
                &mut updates.flags.header.tx8p[i],
                &mut updates.deltas.header.tx8p[i],
            )?;
        }
        for i in 0..2 {
            for j in 0..2 {
                update_probability(
                    &mut reader,
                    &mut probabilities.header.tx16p[i][j],
                    &mut updates.flags.header.tx16p[i][j],
                    &mut updates.deltas.header.tx16p[i][j],
                )?;
            }
        }
        for i in 0..2 {
            for j in 0..3 {
                update_probability(
                    &mut reader,
                    &mut probabilities.header.tx32p[i][j],
                    &mut updates.flags.header.tx32p[i][j],
                    &mut updates.deltas.header.tx32p[i][j],
                )?;
            }
        }
    }

    update_coefficient_probabilities(
        &mut reader,
        tx_mode,
        &mut probabilities.coefficients,
        &reference.coefficients,
        &mut updates.flags.coefficients,
        &mut updates.deltas.coefficients,
    )?;

    for i in 0..3 {
        update_probability(
            &mut reader,
            &mut probabilities.header.skip[i],
            &mut updates.flags.header.skip[i],
            &mut updates.deltas.header.skip[i],
        )?;
    }

    if matches!(header.frame_kind, FrameKind::Key | FrameKind::IntraOnly) {
        return Ok(CompressedHeader {
            tx_mode,
            allow_compound_prediction: false,
            compound_prediction_mode: CompoundPredictionMode::SingleReference,
            probabilities,
            updates,
        });
    }

    for i in 0..7 {
        for j in 0..3 {
            update_probability(
                &mut reader,
                &mut probabilities.header.mv_mode[i][j],
                &mut updates.flags.header.mv_mode[i][j],
                &mut updates.deltas.header.mv_mode[i][j],
            )?;
        }
    }

    if header.interp_filter == 4 {
        for i in 0..4 {
            for j in 0..2 {
                update_probability(
                    &mut reader,
                    &mut probabilities.header.filter[i][j],
                    &mut updates.flags.header.filter[i][j],
                    &mut updates.deltas.header.filter[i][j],
                )?;
            }
        }
    }

    for i in 0..4 {
        update_probability(
            &mut reader,
            &mut probabilities.header.intra[i],
            &mut updates.flags.header.intra[i],
            &mut updates.deltas.header.intra[i],
        )?;
    }

    let allow_compound_prediction = header.ref_frame_sign_bias[0] != header.ref_frame_sign_bias[1]
        || header.ref_frame_sign_bias[0] != header.ref_frame_sign_bias[2];
    let compound_prediction_mode = if allow_compound_prediction {
        let value = if reader.read_bool(128)? {
            if reader.read_bool(128)? {
                CompoundPredictionMode::Select
            } else {
                CompoundPredictionMode::CompoundReference
            }
        } else {
            CompoundPredictionMode::SingleReference
        };
        if value == CompoundPredictionMode::Select {
            for i in 0..5 {
                update_probability(
                    &mut reader,
                    &mut probabilities.header.comp[i],
                    &mut updates.flags.header.comp[i],
                    &mut updates.deltas.header.comp[i],
                )?;
            }
        }
        value
    } else {
        CompoundPredictionMode::SingleReference
    };

    if compound_prediction_mode != CompoundPredictionMode::CompoundReference {
        for i in 0..5 {
            for j in 0..2 {
                update_probability(
                    &mut reader,
                    &mut probabilities.header.single_ref[i][j],
                    &mut updates.flags.header.single_ref[i][j],
                    &mut updates.deltas.header.single_ref[i][j],
                )?;
            }
        }
    }

    if compound_prediction_mode != CompoundPredictionMode::SingleReference {
        for i in 0..5 {
            update_probability(
                &mut reader,
                &mut probabilities.header.comp_ref[i],
                &mut updates.flags.header.comp_ref[i],
                &mut updates.deltas.header.comp_ref[i],
            )?;
        }
    }

    for i in 0..4 {
        for j in 0..9 {
            update_probability(
                &mut reader,
                &mut probabilities.header.y_mode[i][j],
                &mut updates.flags.header.y_mode[i][j],
                &mut updates.deltas.header.y_mode[i][j],
            )?;
        }
    }

    for inverted_index in 0..4 {
        for column in 0..4 {
            for branch in 0..3 {
                let update = reader.read_bool(252)?;
                if update {
                    let update = reader.diff_update_prob(
                        probabilities.header.partition[3 - inverted_index][column][branch],
                    )?;
                    probabilities.header.partition[3 - inverted_index][column][branch] =
                        update.probability;
                    updates.flags.header.partition[3 - inverted_index][column][branch] = 1;
                    updates.deltas.header.partition[3 - inverted_index][column][branch] =
                        update.delta;
                }
            }
        }
    }

    for i in 0..3 {
        update_motion_vector_probability(
            &mut reader,
            &mut probabilities.header.mv_joint[i],
            &mut updates.flags.header.mv_joint[i],
            &mut updates.deltas.header.mv_joint[i],
        )?;
    }

    for component in 0..2 {
        update_motion_vector_probability(
            &mut reader,
            &mut probabilities.header.mv_comp[component].sign,
            &mut updates.flags.header.mv_comp[component].sign,
            &mut updates.deltas.header.mv_comp[component].sign,
        )?;
        for i in 0..10 {
            update_motion_vector_probability(
                &mut reader,
                &mut probabilities.header.mv_comp[component].classes[i],
                &mut updates.flags.header.mv_comp[component].classes[i],
                &mut updates.deltas.header.mv_comp[component].classes[i],
            )?;
        }
        update_motion_vector_probability(
            &mut reader,
            &mut probabilities.header.mv_comp[component].class0,
            &mut updates.flags.header.mv_comp[component].class0,
            &mut updates.deltas.header.mv_comp[component].class0,
        )?;
        for i in 0..10 {
            update_motion_vector_probability(
                &mut reader,
                &mut probabilities.header.mv_comp[component].bits[i],
                &mut updates.flags.header.mv_comp[component].bits[i],
                &mut updates.deltas.header.mv_comp[component].bits[i],
            )?;
        }
    }

    for component in 0..2 {
        for i in 0..2 {
            for j in 0..3 {
                update_motion_vector_probability(
                    &mut reader,
                    &mut probabilities.header.mv_comp[component].class0_fp[i][j],
                    &mut updates.flags.header.mv_comp[component].class0_fp[i][j],
                    &mut updates.deltas.header.mv_comp[component].class0_fp[i][j],
                )?;
            }
        }
        for i in 0..3 {
            update_motion_vector_probability(
                &mut reader,
                &mut probabilities.header.mv_comp[component].fp[i],
                &mut updates.flags.header.mv_comp[component].fp[i],
                &mut updates.deltas.header.mv_comp[component].fp[i],
            )?;
        }
    }

    if header.allow_high_precision_mv {
        for component in 0..2 {
            update_motion_vector_probability(
                &mut reader,
                &mut probabilities.header.mv_comp[component].class0_hp,
                &mut updates.flags.header.mv_comp[component].class0_hp,
                &mut updates.deltas.header.mv_comp[component].class0_hp,
            )?;
            update_motion_vector_probability(
                &mut reader,
                &mut probabilities.header.mv_comp[component].hp,
                &mut updates.flags.header.mv_comp[component].hp,
                &mut updates.deltas.header.mv_comp[component].hp,
            )?;
        }
    }

    Ok(CompressedHeader {
        tx_mode,
        allow_compound_prediction,
        compound_prediction_mode,
        probabilities,
        updates,
    })
}

fn update_probability(
    reader: &mut RangeDecoder<'_>,
    probability: &mut u8,
    flag: &mut u8,
    delta: &mut u8,
) -> Result<(), Vp9Error> {
    if reader.read_bool(252)? {
        let update = reader.diff_update_prob(*probability)?;
        *probability = update.probability;
        *flag = 1;
        *delta = update.delta;
    }
    Ok(())
}

fn update_motion_vector_probability(
    reader: &mut RangeDecoder<'_>,
    probability: &mut u8,
    flag: &mut u8,
    delta: &mut u8,
) -> Result<(), Vp9Error> {
    if reader.read_bool(252)? {
        *probability = ((reader.read_bits(7)? << 1) | 1) as u8;
        *flag = 1;
        *delta = *probability;
    }
    Ok(())
}

fn update_coefficient_probabilities(
    reader: &mut RangeDecoder<'_>,
    tx_mode: TxMode,
    probabilities: &mut CoefficientProbabilities,
    reference: &CoefficientProbabilities,
    flags: &mut CoefficientProbabilities,
    deltas: &mut CoefficientProbabilities,
) -> Result<(), Vp9Error> {
    let transform_count = match tx_mode {
        TxMode::Only4x4 => 1,
        TxMode::Only8x8 => 2,
        TxMode::Only16x16 => 3,
        TxMode::Only32x32 => 4,
        TxMode::Select => 4,
    };

    for transform in 0..transform_count {
        if reader.read_bool(128)? {
            for block_type in 0..2 {
                for intra in 0..2 {
                    for band in 0..6 {
                        for position in 0..6 {
                            if band == 0 && position >= 3 {
                                continue;
                            }
                            for coefficient in 0..3 {
                                let old = reference[transform][block_type][intra][band][position]
                                    [coefficient];
                                let probability = &mut probabilities[transform][block_type][intra]
                                    [band][position][coefficient];
                                if reader.read_bool(252)? {
                                    let update = reader.diff_update_prob(old)?;
                                    *probability = update.probability;
                                    flags[transform][block_type][intra][band][position]
                                        [coefficient] = 1;
                                    deltas[transform][block_type][intra][band][position]
                                        [coefficient] = update.delta;
                                } else {
                                    *probability = old;
                                }
                            }
                        }
                    }
                }
            }
        } else {
            probabilities[transform] = reference[transform];
        }
    }
    Ok(())
}
