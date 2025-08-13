package src

import (
	"encoding/json"
	"fmt"

	wasm "github.com/CosmWasm/wasmd/x/wasm/types"

	"github.com/cosmos/cosmos-sdk/codec"
	"github.com/cosmos/cosmos-sdk/types/tx"
	auth "github.com/cosmos/cosmos-sdk/x/auth/types"
	bank "github.com/cosmos/cosmos-sdk/x/bank/types"
	staking "github.com/cosmos/cosmos-sdk/x/staking/types"
	// sdk "github.com/cosmos/cosmos-sdk/types"
)

// Protobuf message type URLs
const (
	TYPE_URL_WASM_EXECUTE     = "cosmwasm.wasm.v1.MsgExecuteContract"
	TYPE_URL_WASM_INSTANTIATE = "cosmwasm.wasm.v1.MsgInstantiateContract"
	TYPE_URL_WASM_QUERY_SMART = "/cosmwasm.wasm.v1.Query/SmartContractState"
	TYPE_URL_WASM_QUERY_RAW   = "/cosmwasm.wasm.v1.Query/RawContractState"

	TYPE_URL_AUTH_QUERY_ACCOUNT = "/cosmos.auth.v1beta1.Query/Account"

	TYPE_URL_BANK_QUERY_BALANCE      = "/cosmos.bank.v1beta1.Query/Balance"
	TYPE_URL_BANK_QUERY_ALL_BALANCES = "/cosmos.bank.v1beta1.Query/AllBalances"

	TYPE_URL_TX_SIMULATE = "/cosmos.tx.v1beta1.Service/Simulate"
	TYPE_URL_TX_TX       = "/cosmos.tx.v1beta1.Tx"
)

var PB_MSG_MAP map[string]codec.ProtoMarshaler = map[string]codec.ProtoMarshaler{
	// auth, bank, tx
	TYPE_URL_AUTH_QUERY_ACCOUNT:      new(auth.QueryAccountRequest),
	TYPE_URL_BANK_QUERY_BALANCE:      new(bank.QueryBalanceRequest),
	TYPE_URL_BANK_QUERY_ALL_BALANCES: new(bank.QueryAllBalancesRequest),

	TYPE_URL_TX_SIMULATE: new(tx.SimulateRequest),
	TYPE_URL_TX_TX:       new(tx.Tx),

	// cosmwasm
	TYPE_URL_WASM_EXECUTE:     new(wasm.MsgExecuteContract),
	TYPE_URL_WASM_INSTANTIATE: new(wasm.MsgInstantiateContract),
	TYPE_URL_WASM_QUERY_SMART: new(wasm.QuerySmartContractStateRequest),
	TYPE_URL_WASM_QUERY_RAW:   new(wasm.QueryRawContractStateRequest),

	// staking - args: delegatorAddr
	"/cosmos.staking.v1beta1.Query/DelegatorDelegations": new(staking.QueryDelegatorDelegationsRequest),
	"/cosmos.staking.v1beta1.Query/DelegatorValidators":  new(staking.QueryDelegatorValidatorsRequest),
}

var CUSTOM_PROTO_PARSERS map[string]func(protoBz []byte) ([]byte, error) = map[string]func(protoBz []byte) ([]byte, error){
	TYPE_URL_TX_SIMULATE: func(protoBz []byte) ([]byte, error) {
		path := TYPE_URL_TX_SIMULATE
		pbMsg := PB_MSG_MAP[path]
		jsonBz, err := ProtoBzToJsonNoCustom(protoBz, pbMsg)
		if err != nil {
			return nil, err
		}
		var typed struct {
			Tx      []byte `json:"tx"`
			TxBytes string `json:"tx_bytes"`
		}
		err = json.Unmarshal(jsonBz, &typed)
		if err != nil {
			return nil, fmt.Errorf("error unmarshaling JSON to typed: %w", err)
		}
		return PbMsgFromBase64ToJsonNoCustom(
			TYPE_URL_TX_TX, string(typed.TxBytes),
		)
	},
}
