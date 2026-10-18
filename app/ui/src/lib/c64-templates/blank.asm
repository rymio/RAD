; TITLE – a Commodore 64 program in 6502 assembly (64tass syntax).

        * = $0801               ; a BASIC line, 10 SYS 2061, so RUN starts the program
        .word (+), 10
        .null $9e, format("%d", start)
+       .word 0

start   lda #0
        sta $d020               ; border colour
        sta $d021               ; background colour
        ldx #0
        lda #32                 ; clear the screen (1000 characters: 4 × 250)
-       sta $0400,x
        sta $04fa,x
        sta $05f4,x
        sta $06ee,x
        inx
        cpx #250
        bne -
        ldx #0
-       lda message,x           ; copy the message to the top of the screen
        beq loop
        sta $0400,x
        lda #1
        sta $d800,x             ; in white
        inx
        bne -
loop    jmp loop

        .enc "screen"
message .null "HELLO FROM TITLE"
