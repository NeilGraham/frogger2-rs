; ETC command table (reconstructed for f2-etc). Format: see triggers.ec.
; Opcodes and operand layouts follow ExecuteCommand in Common/script.c.
; `On` (token 64) is special-cased by the compiler: On <trigger> <flags> <block>;
; its parameter list here is documentation only.

64	EV_ON				On					(%p_trigger, %p_byte, %p_block)
65	EV_IF				If					(%p_trigger, %p_block)
0	EV_DEBUG			Debug				(%p_string)
2	EV_SETENEMYFLAG		SetEnemyFlag		(%p_word, %p_integer)	; uid, flags
3	EV_RESETENEMYFLAG	ResetEnemyFlag		(%p_word, %p_integer)
4	EV_SETPLATFLAG		SetPlatFlag			(%p_word, %p_integer)
5	EV_RESETPLATFLAG	ResetPlatFlag		(%p_word, %p_integer)
6	EV_ANIMATEACTOR		AnimateActor		(%p_word, %p_byte, %p_byte, %p_float)	; uid, anim, flags, speed
7	EV_SETENEMY			SetEnemy			(%p_word, %p_byte)		; uid, active
8	EV_SETPLATFORM		SetPlatform			(%p_word, %p_byte)
10	EV_MOVEENEMY		MoveEnemy			(%p_word, %p_word)		; uid, node
11	EV_MOVEPLAT			MovePlat			(%p_word, %p_word)
13	EV_CHANGEVOLUME		ChangeVolume		(%p_byte, %p_float, %p_word)
16	EV_CHANGELEVEL		ChangeLevel			(%p_byte, %p_byte)		; world, level
18	EV_HOP				Hop					(%p_byte, %p_word)		; frog, tile
20	EV_FOG				Fog					(%p_byte, %p_byte, %p_byte, %p_word, %p_word)	; r, g, b, min, max
21	EV_NO_FOG			NoFog				(%p_void)
22	EV_CAMEOMODE_ON		CameoModeOn			(%p_void)
23	EV_CAMEOMODE_OFF	CameoModeOff		(%p_void)
24	EV_SETFLAG			SetFlag				(%p_byte)
25	EV_RESETFLAG		ResetFlag			(%p_byte)
27	EV_SFX_ENEMY		SfxEnemy			(%p_word, %p_byte, %p_float, %p_float, %p_float, %p_float)
28	EV_SFX_PLACEHOLDER	SfxPlaceholder		(%p_word, %p_byte, %p_float, %p_float, %p_float, %p_float)
29	EV_HURTFROG			HurtFrog			(%p_void)
30	EV_SCALENMESPEED	ScaleEnemySpeed		(%p_word, %p_float)
31	EV_SCALEPLATSPEED	ScalePlatSpeed		(%p_word, %p_float)
32	EV_INC_COUNTER		IncCounter			(%p_byte)
33	EV_DEC_COUNTER		DecCounter			(%p_byte)
34	EV_SET_COUNTER		SetCounter			(%p_byte, %p_word)
35	EV_COLLAPSE_PLAT	CollapsePlat		(%p_word, %p_float)
36	EV_CHANGEFROG		ChangeFrog			(%p_byte, %p_string)
37	EV_SHAKECAMERA		ShakeCamera			(%p_float, %p_float)
38	EV_SETTILE_P		SetTileP			(%p_word, %p_byte)
39	EV_TELEPORT_P		TeleportP			(%p_byte, %p_word, %p_float)
40	EV_SETSTARTTILE_P	SetStartTileP		(%p_word)
41	EV_SPRING_P			SpringP				(%p_byte, %p_word, %p_float, %p_float)
42	EV_PLAYSOUND		PlaySound			(%p_string, %p_word)
43	EV_STOPSOUND		StopSound			(%p_string)
44	EV_KILLFROG			KillFrog			(%p_byte, %p_byte)
45	EV_ANIMATEFROG		AnimateFrog			(%p_byte, %p_byte, %p_byte, %p_float)
46	EV_RESETSCRIPT		ResetScript			(%p_void)
47	EV_ENDLEVEL			EndLevel			(%p_void)
48	EV_POKEFROGS		PokeFrogs			(%p_void)
49	EV_SFX_P_NOCLIP		SfxPlaceholderNoClip	(%p_word, %p_byte, %p_float, %p_float, %p_float, %p_float)
50	EV_HINT				Hint				(%p_word)
