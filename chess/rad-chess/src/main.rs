//! RAD's chess engine (Games → Chess): one request on stdin, one answer on stdout, as JSON.
//!
//! The search and the opening book are RustChess's (https://github.com/codyjk/RustChess). On top of
//! the best move it says what Gilgamesh needs to explain it: whether it came from the opening book
//! (and the opening's name), the score, the line it expects to follow, and how the other moves
//! scored. Scores are centipawns from White's side (positive: White is better).
//!
//! RustChess's scores favour whichever side moves last at the end of the search (by ½–1 pawn), so
//! RAD asks for even depths: then every score in a game leans the same way, and comparing them
//! (this move against that one, before against after) is fair.
//!
//! Request:  {"fen": null, "moves": ["e2e4", "e7e5"], "depth": 4, "explain": true}
//! Answer:   {"best": "g1f3", "book": false, "opening": "…", "score": 31, "depth": 4, "nodes": 81234,
//!            "ms": 40, "line": ["b8c6", "f1b5"], "candidates": [{"uci": "g1f3", "score": 31}, …]}
//!        or {"error": "…"}

use std::io::{Read, Write};
use std::str::FromStr;
use std::time::Instant;

use chess::alpha_beta_searcher::SearchContext;
use chess::board::Board;
use chess::board::color::Color;
use chess::board::piece::Piece;
use chess::book::{Book, BookMove};
use chess::chess_move::algebraic_notation::enumerate_candidate_moves_with_algebraic_notation;
use chess::chess_move::chess_move::ChessMove;
use chess::chess_search::search_best_move_with_history;
use chess::move_generator::{GameEnding, MoveGenerator, game_ending};
use common::bitboard::Square;
use serde::{Deserialize, Serialize};

/// How much a draw is worth less than nothing to the side searching (as RustChess's own engine).
const CONTEMPT: i16 = 25;
/// RustChess's mate score (`WHITE_WINS`); a found mate scores a little less, by its distance.
const MATE: i16 = i16::MAX / 2;
const LINE_PLIES: usize = 4;

#[derive(Deserialize)]
struct Request {
    fen: Option<String>,
    #[serde(default)]
    moves: Vec<String>,
    depth: Option<u8>,
    #[serde(default)]
    explain: bool,
    /// Plays any move within this many centipawns of the best (an easier opponent).
    #[serde(default)]
    margin: i16,
    /// Play from the opening book while the game is in it.
    #[serde(default = "yes")]
    book: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize)]
struct Candidate {
    uci: String,
    score: i16,
}

#[derive(Serialize, Default)]
struct Answer {
    best: String,
    book: bool,
    opening: Option<String>,
    score: Option<i16>,
    depth: Option<u8>,
    nodes: usize,
    ms: u64,
    line: Vec<String>,
    candidates: Vec<Candidate>,
}

/// A game: the board and every position so far (for repetitions).
struct Game {
    board: Board,
    hashes: Vec<u64>,
    played: Vec<BookMove>,
    movegen: MoveGenerator,
}

impl Game {
    fn new(fen: Option<&str>) -> Result<Self, String> {
        let board = match fen {
            Some(f) => Board::from_str(f).map_err(|e| format!("Invalid FEN: {e:?}"))?,
            None => Board::default(),
        };
        let hashes = vec![board.current_position_hash()];
        Ok(Game { board, hashes, played: Vec::new(), movegen: MoveGenerator::default() })
    }

    fn legal(&mut self) -> Vec<ChessMove> {
        let turn = self.board.turn();
        enumerate_candidate_moves_with_algebraic_notation(&mut self.board, turn, &self.movegen).into_iter().map(|(m, _)| m).collect()
    }

    /// The legal move a UCI string ("e2e4", "e7e8q") stands for.
    fn find(&mut self, uci: &str) -> Result<ChessMove, String> {
        if !(4..=5).contains(&uci.len()) {
            return Err(format!("Invalid move: {uci}"));
        }
        let from = Square::from_algebraic(&uci[0..2]).ok_or_else(|| format!("Invalid square in {uci}"))?;
        let to = Square::from_algebraic(&uci[2..4]).ok_or_else(|| format!("Invalid square in {uci}"))?;
        let promotion = uci.chars().nth(4).map(|c| match c {
            'r' => Piece::Rook,
            'b' => Piece::Bishop,
            'n' => Piece::Knight,
            _ => Piece::Queen,
        });
        self.legal()
            .into_iter()
            .find(|m| {
                m.from_square() == from
                    && m.to_square() == to
                    && match (promotion, m) {
                        (Some(p), ChessMove::PawnPromotion(pm)) => pm.promote_to_piece() == p,
                        (Some(_), _) => false,
                        (None, _) => true,
                    }
            })
            .ok_or_else(|| format!("Illegal move: {uci}"))
    }

    fn play(&mut self, m: &ChessMove) -> Result<(), String> {
        m.apply(&mut self.board).map_err(|e| format!("{e:?}"))?;
        self.board.toggle_turn();
        self.hashes.push(self.board.current_position_hash());
        self.played.push(BookMove::new(m.from_square(), m.to_square()));
        Ok(())
    }

    fn take_back(&mut self, m: &ChessMove) {
        self.board.toggle_turn();
        let _ = m.undo(&mut self.board);
        self.hashes.pop();
        self.played.pop();
    }

    /// How the game stands for the side to move, if it's over: mate is scored as RustChess does.
    fn ended(&mut self) -> Option<i16> {
        let turn = self.board.turn();
        match game_ending(&mut self.board, &self.movegen, turn, &self.hashes)? {
            GameEnding::Checkmate => Some(if turn == Color::White { -MATE } else { MATE }),
            GameEnding::Stalemate | GameEnding::Draw => Some(0),
        }
    }

    /// The best move for the side to move, and its score.
    fn search(&mut self, ctx: &mut SearchContext<ChessMove>) -> Result<(ChessMove, Option<i16>), String> {
        let contempt = if self.board.turn().maximize_score() { -CONTEMPT } else { CONTEMPT };
        let m = search_best_move_with_history(ctx, &mut self.board, self.hashes.clone(), contempt).map_err(|e| format!("{e:?}"))?;
        Ok((m, ctx.last_score()))
    }
}

fn think(req: Request) -> Result<Answer, String> {
    let mut g = Game::new(req.fen.as_deref())?;
    for uci in &req.moves {
        let m = g.find(uci)?;
        g.play(&m)?;
    }
    if g.ended().is_some() {
        return Err("The game is over".into());
    }
    let book = Book::default();
    let mut a = Answer { opening: book.get_line(g.played.clone()), ..Default::default() };
    let depth = req.depth.unwrap_or(4).clamp(2, 10);

    // The opening book first (only from the usual start: its lines are from there).
    let from_book = if req.book && req.fen.is_none() {
        let legal = g.legal();
        let options: Vec<ChessMove> = book
            .get_next_moves(g.played.clone())
            .into_iter()
            .filter_map(|(bm, _)| legal.iter().find(|m| m.from_square() == bm.from_square() && m.to_square() == bm.to_square()).cloned())
            .collect();
        (!options.is_empty()).then(|| options[fastrand_index(options.len())].clone())
    } else {
        None
    };

    // Every move, scored the same way: the reply searched one ply less, so each is seen `depth` deep.
    // RustChess's own root search sometimes returns a move that its deeper look scores poorly;
    // choosing from these scores keeps his move and his explanation of it in agreement.
    let k = depth - 1;
    let mut ctx = SearchContext::new(k);
    let started = Instant::now();
    let mut scored = Vec::new();
    if req.explain || from_book.is_none() {
        for m in g.legal() {
            g.play(&m)?;
            let score = match g.ended() {
                Some(s) => Some(s),
                None => g.search(&mut ctx).ok().and_then(|(_, s)| s),
            };
            a.nodes += ctx.searched_position_count();
            g.take_back(&m);
            if let Some(score) = score {
                scored.push((m, score));
            }
        }
    }
    let white = g.board.turn() == Color::White;
    scored.sort_by_key(|(_, s)| if white { -*s } else { *s });
    a.ms = started.elapsed().as_millis() as u64;

    let best = match from_book {
        Some(m) => {
            a.book = true;
            m
        }
        None => {
            let top = scored.first().map(|(_, s)| *s).ok_or("No legal move")?;
            let near = scored.iter().take_while(|(_, s)| (top - *s).abs() <= req.margin.max(0)).count();
            let (m, s) = scored[fastrand_index(near)].clone();
            a.score = Some(s);
            a.depth = Some(depth);
            m
        }
    };
    a.best = best.to_uci();

    if req.explain {
        a.candidates = scored.iter().take(5).map(|(m, s)| Candidate { uci: m.to_uci(), score: *s }).collect();
        // A book move he chose among the others, wherever it came.
        if !a.candidates.iter().any(|c| c.uci == a.best) {
            if let Some((m, s)) = scored.iter().find(|(m, _)| m.to_uci() == a.best) {
                a.candidates.pop();
                a.candidates.push(Candidate { uci: m.to_uci(), score: *s });
            }
        }
        if a.book {
            a.score = scored.iter().find(|(m, _)| m.to_uci() == a.best).map(|(_, s)| *s);
        }

        // The line he expects: the best replies after his move.
        g.play(&best)?;
        a.opening = book.get_line(g.played.clone()).or(a.opening);
        for _ in 0..LINE_PLIES {
            if g.ended().is_some() {
                break;
            }
            let Ok((m, _)) = g.search(&mut ctx) else { break };
            a.line.push(m.to_uci());
            g.play(&m)?;
        }
    }
    Ok(a)
}

/// A random index below `n`, for picking among the book's moves.
fn fastrand_index(n: usize) -> usize {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    (t as usize ^ std::process::id() as usize) % n
}

fn main() {
    let mut input = String::new();
    let out = match std::io::stdin().read_to_string(&mut input) {
        Ok(_) => match serde_json::from_str::<Request>(&input) {
            Ok(req) => match think(req) {
                Ok(a) => serde_json::to_string(&a).unwrap_or_default(),
                Err(e) => serde_json::json!({ "error": e }).to_string(),
            },
            Err(e) => serde_json::json!({ "error": format!("Bad request: {e}") }).to_string(),
        },
        Err(e) => serde_json::json!({ "error": e.to_string() }).to_string(),
    };
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{out}");
    let _ = stdout.flush();
}
