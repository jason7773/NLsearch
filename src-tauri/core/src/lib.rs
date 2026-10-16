//! Everything NLsearch does with files, the index and the model, apart from running the
//! desktop window and the llama-server process. The app crate wires these together.

pub mod db;
pub mod embed;
pub mod extract;
pub mod index;
pub mod search;
pub mod text;

#[cfg(test)]
mod testing;
