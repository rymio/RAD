; Star Catch – a Commodore 64 game in 6502 assembly (64tass syntax).
; Move the catcher with a joystick in port 2 (or the Z and X keys) and catch the falling
; stars. Miss three and the game starts again.

        * = $0801               ; a BASIC line, 10 SYS 2061, so RUN starts the game
        .word (+), 10
        .null $9e, format("%d", start)
+       .word 0

SCREEN  = $0400                 ; 40 × 25 characters (screen codes)
COLORS  = $d800                 ; a colour for each character
ROW     = 23                    ; the catcher's row
WIDTH   = 3                     ; the catcher's width in characters
ptr     = $fb                   ; zero page: a screen address ($fb–$fe are free)
cptr    = $fd                   ; ...and its colour RAM address

start   sei                     ; no KERNAL interrupts: the game owns the machine
        lda #$7f
        sta $dc0d               ; CIA 1 interrupts off
        lda $d012
        ora #1
        sta seed                ; a random seed from the raster position
        lda #0
        sta $d020               ; black border
        sta $d021               ; black background
        lda #$15
        sta $d018               ; upper case and graphics characters

restart ldx #$ff
        txs                     ; a fresh stack for every game
        jsr clear
        lda #3
        sta lives
        lda #0
        sta score
        lda #18
        sta xpos
        lda #8
        sta speed
        sta timer
        lda #4
        sta level
        lda #1
        sta movet
        jsr newstar
        jsr status

main    jsr frame               ; 50 times a second
        jsr input
        jsr catcher
        dec timer
        bne main
        lda speed
        sta timer
        jsr fall
        jmp main

; Waits for the next frame (the raster beam passing line 251).
frame   lda $d012
        cmp #251
        bne frame
-       lda $d012
        cmp #251
        beq -
        rts

; Moves the catcher: joystick 2 (bit 2 left, bit 3 right; 0 = pushed), or Z / X.
input   lda #$ff
        sta $dc00               ; no keyboard row selected: $dc00 reads joystick 2
        lda $dc00
        sta joy
        lda #%11111101          ; keyboard row 1: Z is column 4
        sta $dc00
        lda $dc01
        and #%00010000
        bne +
        lda joy
        and #%11111011
        sta joy
+       lda #%11111011          ; keyboard row 2: X is column 7
        sta $dc00
        lda $dc01
        and #%10000000
        bne +
        lda joy
        and #%11110111
        sta joy
+       lda #$ff
        sta $dc00
        dec movet               ; one step every 3 frames
        bne done
        lda #3
        sta movet
        lda joy
        and #%00000100
        bne +
        lda xpos
        beq +
        dec xpos
+       lda joy
        and #%00001000
        bne done
        lda xpos
        cmp #40-WIDTH
        bcs done
        inc xpos
done    rts

; Draws the catcher ("<=>") on its row.
catcher ldx #39
        lda #32
-       sta SCREEN+ROW*40,x
        dex
        bpl -
        ldx xpos
        lda #$3c
        sta SCREEN+ROW*40,x
        lda #$3d
        sta SCREEN+ROW*40+1,x
        lda #$3e
        sta SCREEN+ROW*40+2,x
        rts

; The star falls one row; at the catcher's row it's caught or missed.
fall    jsr starptr
        ldy #0
        lda #32
        sta (ptr),y
        inc sy
        lda sy
        cmp #ROW
        beq landed
        jsr starptr
        ldy #0
        lda #$2a                ; "*"
        sta (ptr),y
        lda #7                  ; yellow
        sta (cptr),y
        rts

landed  lda sx
        sec
        sbc xpos
        cmp #WIDTH
        bcs missed              ; left of the catcher wraps round to a big number too
        sed                     ; the score counts in decimal (BCD)
        lda score
        clc
        adc #1
        sta score
        cld
        dec level               ; faster every 4 stars
        bne +
        lda #4
        sta level
        lda speed
        cmp #3
        bcc +
        dec speed
+       jsr status
        jmp newstar

missed  dec lives
        jsr status
        lda lives
        bne newstar
        ldx #0                  ; game over: a message, three seconds, start again
-       lda over,x
        beq +
        sta SCREEN+12*40+15,x
        inx
        bne -
+       ldy #150
-       jsr frame
        dey
        bne -
        jmp restart

; A new star at the top, in a random column (0–39).
newstar lda seed
        asl a
        bcc +
        eor #$1d
+       sta seed
-       cmp #40
        bcc +
        sbc #40
        bcs -
+       sta sx
        lda #2
        sta sy
        rts

; ptr / cptr = where the star is on the screen / in colour RAM.
starptr ldx sy
        lda rowlo,x
        clc
        adc sx
        sta ptr
        sta cptr
        lda rowhi,x
        adc #0
        sta ptr+1
        clc
        adc #>(COLORS-SCREEN)
        sta cptr+1
        rts

; The top line: title, score (two BCD digits) and lives.
status  ldx #TITLELEN-1
-       lda title,x
        sta SCREEN,x
        lda #1
        sta COLORS,x
        dex
        bpl -
        lda score
        lsr a
        lsr a
        lsr a
        lsr a
        ora #$30
        sta SCREEN+SCOREAT
        lda score
        and #$0f
        ora #$30
        sta SCREEN+SCOREAT+1
        lda lives
        ora #$30
        sta SCREEN+LIVESAT
        rts

; Clears the screen: spaces, white, the catcher's row yellow.
clear   ldx #0
-       lda #32
        sta SCREEN,x
        sta SCREEN+$100,x
        sta SCREEN+$200,x
        sta SCREEN+$2e8,x
        lda #1
        sta COLORS,x
        sta COLORS+$100,x
        sta COLORS+$200,x
        sta COLORS+$2e8,x
        inx
        bne -
        ldx #39
        lda #7
-       sta COLORS+ROW*40,x
        dex
        bpl -
        rts

; Each screen row's address.
rowlo   .for i := 0, i < 25, i += 1
        .byte <(SCREEN + i*40)
        .endfor
rowhi   .for i := 0, i < 25, i += 1
        .byte >(SCREEN + i*40)
        .endfor

        .enc "screen"           ; text as screen codes
title   .text "STAR CATCH    SCORE 00    LIVES 3"
TITLELEN = * - title
SCOREAT = 20
LIVESAT = 32
over    .null "GAME OVER"
        .enc "none"

; Variables.
xpos    .byte 0
sx      .byte 0
sy      .byte 0
score   .byte 0
lives   .byte 0
speed   .byte 0
timer   .byte 0
level   .byte 0
movet   .byte 0
joy     .byte 0
seed    .byte 0
