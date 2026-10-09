; ETC trigger table (reconstructed for f2-etc).
;
; Format, one entry per trigger (same as the original ETC tables):
;   <token 0-255> <C #define name> <script name> ( <param type>, ... )
; Param types: %p_void (no params), %p_byte, %p_word, %p_integer,
; %p_float (16.16 in the file), %p_string, %p_trigger, %p_block.
; Entry order matters only for the order of `codes.h` (written in reverse).
; Opcodes and operand layouts follow LoadTrigger in Common/script.c.

0	TR_ENEMYONTILE		EnemyOnTile		(%p_word, %p_word)		; enemy uid, tile
1	TR_FROGONTILE		FrogOnTile		(%p_byte, %p_word)		; frog, tile
2	TR_FROGONPLATFORM	FrogOnPlatform	(%p_byte, %p_word)		; frog, platform uid
3	TR_RANDOMWAIT		RandomWait		(%p_float, %p_float)	; min secs, max secs
8	TR_ENEMYATFLAG		EnemyAtFlag		(%p_word, %p_word)		; enemy uid, path node
9	TR_PLATATFLAG		PlatAtFlag		(%p_word, %p_word)		; platform uid, path node
10	TR_WAIT				Wait			(%p_float)				; seconds
11	TR_FLAGSET			FlagSet			(%p_byte)				; script flag
12	TR_COUNTEREQUALS	CounterEquals	(%p_byte, %p_word)		; counter, value
13	TR_FROGISDEAD		FrogIsDead		(%p_byte)				; frog
14	TR_LEVELISOPEN		LevelIsOpen		(%p_byte, %p_byte)		; world, level
15	TR_ONPATH			OnPath			(%p_byte, %p_word)		; frog, enemy uid
64	TR_AND				And				(%p_trigger, %p_trigger)
65	TR_OR				Or				(%p_trigger, %p_trigger)
66	TR_NOT				Not				(%p_trigger)
