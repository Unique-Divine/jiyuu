package src

import (
	"github.com/NibiruChain/nibiru/v2/app"
	"github.com/cosmos/cosmos-sdk/codec"
)

// Global variable to hold a ProtoCodec instance for encoding/decoding protocol
// buffer messages.
var PROTO_CODEC *codec.ProtoCodec

// init runs before the rest of program execution.
func init() {
	// initializes the ProtoCodec used for encoding/decoding in the application.
	encCfg := app.MakeEncodingConfig()
	iReg := encCfg.InterfaceRegistry
	PROTO_CODEC = codec.NewProtoCodec(iReg)
}
