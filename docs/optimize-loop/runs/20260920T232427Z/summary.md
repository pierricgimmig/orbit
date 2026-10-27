# Optimize-loop suite — stockfish

- time: 2026-09-20T23:24:27Z
- project: `/home/pierric/git/stockfish`
- config: `/tmp/claude-1000/-home-pierric-git-perfr/5caa4de1-4a11-4b27-bd97-ea62ebcdf1ad/scratchpad/stockfish.toml`
- metric: **nps** mean 1441316.5 ± 2974.335724157581 (4×)
- fingerprint: 1659671
- correctness: True
- profile backend: perfr-http

| % self | symbol |
| ---: | --- |
| 36.41 | `Stockfish::Eval::NNUE::(anonymous namespace)::apply_combined(Stockfish::Color, Stockfish::Eval::NNUE::FeatureTransformer const&, Stockfish::Eval::NNUE::AccumulatorState const&, Stockfish::Eval::NNUE::AccumulatorState&, Stockfish::ValueList<unsigned short, 32ul> const&, Stockfish::ValueList<unsigned short, 32ul> const&, Stockfish::ValueList<unsigned short, 256ul> const&, Stockfish::ValueList<unsigned short, 256ul> const&)` |
| 9.54 | `Stockfish::Eval::NNUE::NetworkArchitecture::propagate(unsigned char const*, Stockfish::Eval::NNUE::NNZInfo<1024ul> const&) const` |
| 7.52 | `int Stockfish::Search::Worker::search<(Stockfish::NodeType)0>(Stockfish::Position&, Stockfish::Search::Stack*, int, int, int, bool)` |
| 6.34 | `Stockfish::MovePicker::next_move()` |
| 5.85 | `Stockfish::Eval::NNUE::(anonymous namespace)::update_accumulator_refresh_cache(Stockfish::Color, Stockfish::Eval::NNUE::FeatureTransformer const&, Stockfish::Position const&, Stockfish::Eval::NNUE::AccumulatorState&, Stockfish::Eval::NNUE::AccumulatorCaches&)` |
| 3.43 | `Stockfish::Eval::NNUE::(anonymous namespace)::update_accumulator_incremental_both(Stockfish::Eval::NNUE::FeatureTransformer const&, Stockfish::Square, Stockfish::Square, Stockfish::Eval::NNUE::AccumulatorState&, Stockfish::Eval::NNUE::AccumulatorState const&)` |
| 3.37 | `Stockfish::(anonymous namespace)::partial_insertion_sort(Stockfish::ExtMove*, Stockfish::ExtMove*, int)` |
| 2.36 | `void Stockfish::Position::update_piece_threats<true>(Stockfish::Piece, bool, Stockfish::Square, Stockfish::DirtyThreats*, unsigned long) const` |
| 2.15 | `Stockfish::TranspositionTable::probe(unsigned long) const` |
| 2.08 | `Stockfish::Eval::NNUE::(anonymous namespace)::update_accumulator_hybrid(Stockfish::Color, Stockfish::Position const&, Stockfish::Eval::NNUE::FeatureTransformer const&, Stockfish::Eval::NNUE::AccumulatorState&, Stockfish::Eval::NNUE::AccumulatorState const&, Stockfish::Eval::NNUE::AccumulatorCaches&)` |
| 1.99 | `Stockfish::Position::do_move(Stockfish::Move, Stockfish::StateInfo&, bool, Stockfish::Dirties&, Stockfish::TranspositionTable const*, Stockfish::SharedHistories const*)` |
| 1.94 | `Stockfish::(anonymous namespace)::correction_value(Stockfish::Search::Worker const&, Stockfish::Position const&, Stockfish::Search::Stack const*)` |
| 1.77 | `Stockfish::Eval::NNUE::Network::evaluate(Stockfish::Position const&, Stockfish::Eval::NNUE::AccumulatorStack&, Stockfish::Eval::NNUE::AccumulatorCaches&) const` |
| 1.56 | `Stockfish::Position::see_ge(Stockfish::Move, int) const` |
| 1.06 | `Stockfish::Search::Worker::do_move(Stockfish::Position&, Stockfish::Move, Stockfish::StateInfo&, bool, Stockfish::Search::Stack*)` |

