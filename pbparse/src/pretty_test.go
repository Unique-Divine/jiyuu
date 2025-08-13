package src_test

import (
	"encoding/json"
	"testing"

	"github.com/Unique-Divine/jiyuu/pbparse/src"
	"github.com/stretchr/testify/suite"
)

// --------------------------------------------------
// Suite Setup
// --------------------------------------------------

// Runs all of the tests in each suite.
func Test(t *testing.T) {
	suite.Run(t, new(Suite))
}

type Suite struct {
	suite.Suite
}

// --------------------------------------------------
// Start of Tests
// --------------------------------------------------

func (s *Suite) TestPrettyPrintJSON() {
	// Extracted from Eris.json Data
	jsonData := []byte(`[
  {
    "jsonrpc": "2.0",
    "id": 228434741417,
    "method": "abci_query",
    "params": {
      "path": "/cosmwasm.wasm.v1.Query/SmartContractState",
      "data": "{\"address\":\"nibi1udqqx30cw8nwjxtl4l28ym9hhrp933zlq8dqxfjzcdhvl8y24zcqpzmh8m\",\"query_data\":{\"state\":{}}}",
      "prove": false
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 956687516565,
    "method": "abci_query",
    "params": {
      "path": "/cosmos.staking.v1beta1.Query/DelegatorDelegations",
      "data": "{\"delegator_addr\":\"nibi1udqqx30cw8nwjxtl4l28ym9hhrp933zlq8dqxfjzcdhvl8y24zcqpzmh8m\",\"pagination\":null}",
      "prove": false
    }
  }
]`)

	var reqs []src.JsonRpcReq
	err := json.Unmarshal(jsonData, &reqs)
	s.NoError(err)

	{
		pretty, err := src.PrettyJSON(reqs[0].Params.Data)
		s.NoError(err)
		s.Require().Equal(`{
  "address": "nibi1udqqx30cw8nwjxtl4l28ym9hhrp933zlq8dqxfjzcdhvl8y24zcqpzmh8m",
  "query_data": {
    "state": {}
  }
}`, pretty)
	}

	{
		pretty, err := src.PrettyJSON(reqs[1].Params.Data)
		s.NoError(err)
		s.Require().Equal(`{
  "delegator_addr": "nibi1udqqx30cw8nwjxtl4l28ym9hhrp933zlq8dqxfjzcdhvl8y24zcqpzmh8m",
  "pagination": null
}`, pretty)
	}
}
