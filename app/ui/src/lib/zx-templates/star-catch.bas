10 REM Star Catch: catch the falling stars. O = left, P = right
20 BORDER 0: PAPER 0: INK 7: CLS
30 LET x=14: LET sc=0: LET lives=3
40 LET sx=INT (RND*30)+1: LET sy=2
50 GO SUB 500
60 PRINT AT 21,0; INK 4;"O = left    P = right"
100 LET k$=INKEY$
110 IF k$="o" AND x>0 THEN PRINT AT 20,x+2;" ": LET x=x-1
120 IF k$="p" AND x<29 THEN PRINT AT 20,x;" ": LET x=x+1
130 PRINT AT 20,x; INK 5;"=^="
140 PRINT AT sy,sx;" "
150 LET sy=sy+1
160 IF sy=20 THEN GO SUB 300
170 PRINT AT sy,sx; INK 6; BRIGHT 1;"*"
180 GO TO 100
300 REM the star reached the bottom
310 IF sx>=x AND sx<=x+2 THEN LET sc=sc+1: BEEP .02,24: GO TO 340
320 LET lives=lives-1: BEEP .2,-12
330 IF lives=0 THEN GO TO 400
340 PRINT AT sy,sx;" "
350 LET sy=2: LET sx=INT (RND*30)+1
360 GO SUB 500
370 RETURN
400 PRINT AT 10,11; INK 2; FLASH 1;"GAME OVER"
410 PRINT AT 12,5;"Press any key to play"
420 PAUSE 0: GO TO 20
500 PRINT AT 0,0; INK 6;"SCORE ";sc;"  "; AT 0,22; INK 2;"LIVES ";lives
510 RETURN
