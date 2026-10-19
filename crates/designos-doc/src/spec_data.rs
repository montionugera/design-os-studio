//! Document data (besides nodes) that `apply_spec` can write: variables, collections, styles,
//! style metadata and library bookkeeping. Used to roll a failed apply back and to commit a
//! successful one as undoable commands.

use crate::{Command, Document};

impl Document {
    /// Commands that bring `self`'s variables, styles and library state to those of `applied`
    /// (a copy of `self` that `apply_spec` ran on), one per group that differs; empty when the
    /// apply wrote no such data. Run them before inserting the nodes so one undo step reverts both.
    pub fn spec_data_commands(&self, applied: &Document) -> Vec<Command> {
        let mut cmds = Vec::new();
        if self.variables != applied.variables || self.variable_collections != applied.variable_collections {
            cmds.push(Command::SetVariables { variables: applied.variables.clone(), collections: applied.variable_collections.clone() });
        }
        if self.styles != applied.styles || self.style_meta != applied.style_meta {
            cmds.push(Command::SetStyles { styles: applied.styles.clone(), style_meta: applied.style_meta.clone() });
        }
        if self.library != applied.library || self.library_page != applied.library_page {
            cmds.push(Command::SetLibraryState { state: applied.library.clone(), library_page: applied.library_page });
        }
        cmds
    }
}
