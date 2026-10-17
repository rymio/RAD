; Bat & Ball: a tiny Spectrum game in Z80 assembly.
; O = left, P = right (or a Kempston joystick). Built by RAD with sjasmplus: RAD adds the
; machine (DEVICE ZXSPECTRUM48) and makes a snapshot that starts at `start`.

        ORG $8000

ATTR    EQU $5800           ; colour memory: 32 x 24 cells, one byte each
BAT_W   EQU 5
EMPTY   EQU %00000000       ; black
WALL    EQU %00111000       ; white paper, black ink
BAT     EQU %01101000       ; bright cyan
BALL    EQU %01010000       ; bright red

start:
        di
        ld sp,$ff00
        im 1                ; the ROM's interrupt: HALT waits for the next frame (1/50 s)
        ei
restart:
        xor a
        out ($fe),a         ; black border
        ld (score),a
        call clear
        call walls
        ld hl,title
        ld b,0
        ld c,1
        call print
        call show_score
        ld a,13
        ld (bat_x),a
        ld a,16
        ld (ball_x),a
        ld a,4
        ld (ball_y),a
        ld a,1
        ld (dx),a
        ld (dy),a
        call input
main:
        halt
        ld a,(frame)
        inc a
        ld (frame),a
        bit 0,a
        call z,input        ; the bat moves every 2nd frame
        ld a,(frame)
        and 3
        call z,move_ball    ; the ball every 4th frame
        jr main

; ---------------------------------------------------------------- the bat

input:
        ld a,$df            ; keyboard half-row Y U I O P
        in a,($fe)
        cpl                 ; 1 = pressed: bit 0 = P, bit 1 = O
        ld c,a
        in a,($1f)          ; Kempston joystick: bit 0 right, bit 1 left
        cp $ff              ; no joystick answers $ff
        jr z,.keys
        or c
        ld c,a
.keys:
        ld a,(bat_x)
        bit 1,c
        jr z,.notleft
        cp 1
        jr z,.notleft
        dec a
.notleft:
        bit 0,c
        jr z,.notright
        cp 31-BAT_W
        jr z,.notright
        inc a
.notright:
        ld (bat_x),a
        ld b,22             ; clear the bat's row, then draw it
        ld c,1
        call attr_addr
        ld b,30
.clr:   ld (hl),EMPTY
        inc hl
        djnz .clr
        ld b,22
        ld a,(bat_x)
        ld c,a
        call attr_addr
        ld b,BAT_W
.bat:   ld (hl),BAT
        inc hl
        djnz .bat
        ret

; ---------------------------------------------------------------- the ball

move_ball:
        ld a,(ball_y)
        ld b,a
        ld a,(ball_x)
        ld c,a
        call attr_addr
        ld (hl),EMPTY       ; rub out the ball
        ld a,(ball_x)       ; bounce off the side walls
        cp 1
        jr nz,.notleftwall
        ld a,1
        ld (dx),a
.notleftwall:
        ld a,(ball_x)
        cp 30
        jr nz,.notrightwall
        ld a,-1
        ld (dx),a
.notrightwall:
        ld a,(ball_y)       ; and the top
        cp 1
        jr nz,.nottop
        ld a,1
        ld (dy),a
.nottop:
        ld a,(ball_y)       ; just above the bat, going down: hit or miss?
        cp 21
        jr nz,.move
        ld a,(dy)
        cp 1
        jr nz,.move
        ld a,(bat_x)
        ld b,a
        ld a,(ball_x)
        sub b
        cp BAT_W            ; 0 .. BAT_W-1 is on the bat
        jr nc,.move
        ld a,-1
        ld (dy),a
        ld hl,score
        inc (hl)
        call show_score
        call beep
.move:
        ld a,(ball_x)
        ld hl,dx
        add a,(hl)
        ld (ball_x),a
        ld a,(ball_y)
        ld hl,dy
        add a,(hl)
        ld (ball_y),a
        cp 23
        jr z,game_over
        ld b,a
        ld a,(ball_x)
        ld c,a
        call attr_addr
        ld (hl),BALL
        ret

game_over:
        pop hl              ; we came here from `call z,move_ball`
        ld b,100
.flash: ld a,b              ; flash the border for 2 seconds
        and 7
        out ($fe),a
        halt
        djnz .flash
        jp restart

; ---------------------------------------------------------------- screen

; HL = the colour cell of row B, column C
attr_addr:
        ld l,b
        ld h,0
        add hl,hl
        add hl,hl
        add hl,hl
        add hl,hl
        add hl,hl
        ld a,l
        add a,c
        ld l,a
        ld a,h
        adc a,$58
        ld h,a
        ret

clear:
        ld hl,$4000         ; pixels
        ld de,$4001
        ld bc,6143
        ld (hl),0
        ldir
        ld hl,ATTR          ; colours
        ld de,ATTR+1
        ld bc,767
        ld (hl),EMPTY
        ldir
        ret

walls:
        ld hl,ATTR          ; the top row
        ld b,32
.top:   ld (hl),WALL
        inc hl
        djnz .top
        ld hl,ATTR          ; the sides
        ld de,32
        ld b,24
.side:  ld (hl),WALL
        push hl
        ld a,l
        add a,31
        ld l,a
        ld a,h
        adc a,0
        ld h,a
        ld (hl),WALL
        pop hl
        add hl,de
        djnz .side
        ret

; Prints the 0-terminated text at HL from row B, column C (with the ROM's font).
print:
        ld a,(hl)
        or a
        ret z
        push hl
        push bc
        call char
        pop bc
        pop hl
        inc hl
        inc c
        jr print

; Draws character A at row B, column C.
char:
        ld l,a
        ld h,0
        add hl,hl
        add hl,hl
        add hl,hl
        ld de,$3c00         ; the ROM font (character 32 is at $3d00)
        add hl,de
        ex de,hl
        ld a,b              ; screen address of the cell's top pixel row
        and $18
        or $40
        ld h,a
        ld a,b
        and 7
        rrca
        rrca
        rrca
        or c
        ld l,a
        ld b,8
.row:   ld a,(de)
        ld (hl),a
        inc de
        inc h
        djnz .row
        ret

show_score:
        ld a,(score)
        ld b,0
.tens:  cp 10
        jr c,.ones
        sub 10
        inc b
        jr .tens
.ones:  add a,'0'
        ld (score_text+7),a
        ld a,b
        add a,'0'
        ld (score_text+6),a
        ld hl,score_text
        ld b,0
        ld c,22
        jp print

; Short click on the speaker (bit 4 of port $fe).
beep:
        ld b,40
.tone:  ld a,$10
        out ($fe),a
        ld c,30
.d1:    dec c
        jr nz,.d1
        xor a
        out ($fe),a
        ld c,30
.d2:    dec c
        jr nz,.d2
        djnz .tone
        ret

title:      db "BAT & BALL  O/P",0
score_text: db "SCORE 00",0
bat_x:      db 0
ball_x:     db 0
ball_y:     db 0
dx:         db 0
dy:         db 0
frame:      db 0
score:      db 0
