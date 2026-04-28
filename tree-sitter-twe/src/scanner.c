// External scanner for indentation-sensitive Twe.
//
// Twe (like Python and GDScript) uses indentation to structure
// blocks instead of `{` and `}`. Tree-sitter's generated lexer
// can't track an indent stack on its own, so we hand-roll one
// here and expose three virtual tokens to grammar.js:
//
//   _newline -- end of a logical line at the same indent level.
//   _indent  -- start of a deeper-indented block.
//   _dedent  -- end of a block (one per level closed).
//
// Newlines inside `(...)`, `[...]`, and interpolated `"...{}..."`
// strings are handled by the grammar: those rules don't expect
// `_newline` as a valid token, so when valid_symbols[NEWLINE]
// is false the scanner returns false and the generated lexer
// skips whitespace normally.
//
// Modeled closely on tree-sitter-python's scanner.

#include "tree_sitter/parser.h"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wctype.h>

enum TokenType {
    NEWLINE,
    INDENT,
    DEDENT,
};

// Indent stack: each entry is a column count. The stack always
// contains at least one entry (0) representing the script-level
// indent. Pushed on INDENT, popped on DEDENT. 64 is far more
// than any human-readable program would ever nest.
#define MAX_INDENTS 64

typedef struct {
    uint16_t stack[MAX_INDENTS];
    uint8_t depth; // current top-of-stack index (0 = no nesting)
} Scanner;

static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

void *tree_sitter_twe_external_scanner_create(void) {
    Scanner *scanner = (Scanner *)calloc(1, sizeof(Scanner));
    scanner->stack[0] = 0;
    scanner->depth = 0;
    return scanner;
}

void tree_sitter_twe_external_scanner_destroy(void *payload) {
    free(payload);
}

unsigned tree_sitter_twe_external_scanner_serialize(void *payload, char *buffer) {
    Scanner *s = (Scanner *)payload;
    unsigned size = 0;
    buffer[size++] = (char)s->depth;
    for (unsigned i = 0; i <= s->depth; i++) {
        if (size + 2 > TREE_SITTER_SERIALIZATION_BUFFER_SIZE) break;
        buffer[size++] = (char)(s->stack[i] & 0xFF);
        buffer[size++] = (char)((s->stack[i] >> 8) & 0xFF);
    }
    return size;
}

void tree_sitter_twe_external_scanner_deserialize(
    void *payload, const char *buffer, unsigned length
) {
    Scanner *s = (Scanner *)payload;
    s->depth = 0;
    s->stack[0] = 0;
    if (length == 0) return;
    unsigned cursor = 0;
    s->depth = (uint8_t)buffer[cursor++];
    for (unsigned i = 0; i <= s->depth && cursor + 2 <= length; i++) {
        s->stack[i] = (uint16_t)((unsigned char)buffer[cursor]) |
                      ((uint16_t)((unsigned char)buffer[cursor + 1]) << 8);
        cursor += 2;
    }
}

bool tree_sitter_twe_external_scanner_scan(
    void *payload, TSLexer *lexer, const bool *valid_symbols
) {
    Scanner *s = (Scanner *)payload;

    bool found_end_of_line = false;
    uint32_t indent_length = 0;

    for (;;) {
        if (lexer->lookahead == 0) {
            // End of file. Treat as a logical line terminator
            // followed by a dedent back to script level so any
            // open blocks close cleanly.
            indent_length = 0;
            found_end_of_line = true;
            break;
        }
        if (lexer->lookahead == '\n') {
            found_end_of_line = true;
            indent_length = 0;
            skip(lexer);
        } else if (lexer->lookahead == '\r') {
            skip(lexer);
        } else if (lexer->lookahead == ' ') {
            indent_length++;
            skip(lexer);
        } else if (lexer->lookahead == '\t') {
            // Tabs as 8-column for indent comparison. Twe canonical
            // style is 4-space indent (enforced by `twec fmt`); this
            // is only here so the parser tolerates non-canonical
            // sources.
            indent_length += 8;
            skip(lexer);
        } else if (!found_end_of_line) {
            // First column on a non-blank line — nothing for us
            // to do; let the generated lexer handle it.
            return false;
        } else if (lexer->lookahead == '#') {
            // Skip comment to end of line; keep looking for a
            // logical newline.
            while (lexer->lookahead != 0 && lexer->lookahead != '\n') {
                skip(lexer);
            }
        } else {
            break;
        }
    }

    if (found_end_of_line) {
        if (valid_symbols[INDENT] && indent_length > s->stack[s->depth]) {
            if (s->depth + 1 >= MAX_INDENTS) return false;
            s->depth++;
            s->stack[s->depth] = (uint16_t)indent_length;
            lexer->result_symbol = INDENT;
            return true;
        }

        if (valid_symbols[DEDENT] && indent_length < s->stack[s->depth]) {
            s->depth--;
            lexer->result_symbol = DEDENT;
            return true;
        }

        if (valid_symbols[NEWLINE]) {
            // Emit NEWLINE even at EOF — the last statement in a
            // file still needs a terminator or the parser produces
            // an UNEXPECTED \n / EOF error. INDENT/DEDENT win above
            // when the column changed, so we only reach here when
            // the next non-blank line is at the same level or it's
            // the end of input.
            lexer->result_symbol = NEWLINE;
            return true;
        }
    }

    return false;
}
