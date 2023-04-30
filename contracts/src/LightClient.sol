pragma solidity 0.8.17;

import "solidity-merkle-trees/src/MerklePatricia.sol";
import { EventDecoder } from "src/EventDecoder.sol";
import { NUM_AUTHORITIES, GRANDPA_AUTHORITIES_SETID_KEY, SYSTEM_EVENTS_KEY } from "src/Constants.sol";


struct Groth16Proof {
    uint256[2] a;
    uint256[2][2] b;
    uint256[2] c;
}


// Storage value and proof
struct AuthoritySetIDProof {
    uint64 authoritySetID;
    bytes[] merkleProof;  // Proof that it's within the state root.
}


// Storage value and proof
struct EventListProof {
    bytes encodedEventList;
    bytes[] merkleProof; // Proof that it's within the state root.
}


struct Header {
    uint32 blockNumber;
    bytes32 headerHash;
    bytes32 stateRoot;
    bytes32 dataRoot;
}


struct Step {
    Header[] headers;

    // This field specifies and proves the last header's authority set id.
    // Note that this is proven aginst the state root of the 2nd to last header (which may already be saved in the smart contract's state).
    // Note that we can move this verfiication logic into the proof field, if we need to save on the gas.
    AuthoritySetIDProof authoritySetIDProof;

    // This proof is used to verify the following:
    // 1) There exists a sequence of block headers that have the following properties:
    //     a) Those headers are chained together via the parent_hash field and have incremental block numbers.
    //     b) The first header has the block number and parent hash that is stored in the smart contract.
    //     c) Those headers have the submitted headerRoots (basically that those roots are the blake2 digest of those headers).
    //     d) Those headers have the submitted executionStateRoots and dataRoots.
    // 2) There exist a valid GRANDPA justification that finalized the last block in the headers field
    //     a) This GRANDPA justification has been signed by the validators within the authority set ID within the authoritySetIDProof field.
    //Groth16Proof proof;
}


// This is used to update the light client's authority set.
// Note that the verification logic is currently done purely in solidity since the Avail testnet's authority set is small,
// but this will need to be converted into a snark proof.
struct Rotate {
    // This field specifies and proves the scale encoded systems::events list for the block (this will contain the NewAuthorities event).
    EventListProof eventListProof;

    // This field specifies and proves the new authority set's ID (proved against the state root of the blockNumber).
    AuthoritySetIDProof newAuthoritySetIDProof;

    // This field updates the light client's headers up to Rotate.blocknumber.
    Step step;
}


/// @title Light Client for Avail Blockchain
/// @author Succinct Labs
/// @notice Uses Substrate's BABE and GRANDPA protocol to keep up-to-date with block headers from
///         the Avail blockchain. This is done in a gas-efficient manner using zero-knowledge proofs.
contract LightClient is EventDecoder {
    uint256 public immutable START_CHECKPOINT_BLOCK_NUMBER;
    bytes32 public immutable START_CHECKPOINT_HEADER_HASH;

    /// @notice The latest block_number the light client has a finalized header for.
    uint32 public head;

    /// @notice The active authority set ID
    uint64 public activeAuthoritySetID;

    /// @notice Maps from a block number to an Avail header hash.
    mapping(uint32 => bytes32) public headerHashes;

    /// @notice Maps from a block number to the state root.
    mapping(uint32 => bytes32) public stateRoots;

    /// @notice Maps from a block number to the data root.
    mapping(uint32 => bytes32) public dataRoots;

    /// @notice Maps from a authority set id to the authorities' pub keys
    mapping(uint64 => bytes32[NUM_AUTHORITIES]) public authoritySets;

    event HeadUpdate(uint32 indexed blockNumber, bytes32 indexed root);
    event AuthoritySetUpdate(uint64 indexed authoritySetID);

    constructor(
        uint64 startCheckpointAuthoritySetID,
        bytes32[NUM_AUTHORITIES] memory startCheckpointAuthorities,
        Header memory startCheckpointHeader
    ) {
        START_CHECKPOINT_BLOCK_NUMBER = startCheckpointHeader.blockNumber;
        START_CHECKPOINT_HEADER_HASH = startCheckpointHeader.headerHash;
        headerHashes[startCheckpointHeader.blockNumber] = startCheckpointHeader.headerHash;
        stateRoots[startCheckpointHeader.blockNumber] = startCheckpointHeader.stateRoot;
        dataRoots[startCheckpointHeader.blockNumber] = startCheckpointHeader.dataRoot;
        head = startCheckpointHeader.blockNumber;
        emit HeadUpdate(head, startCheckpointHeader.headerHash);

        setAuthorities(startCheckpointAuthoritySetID, startCheckpointAuthorities);
    }

    function setAuthorities(uint64 authoritySetID, bytes32[NUM_AUTHORITIES] memory _authorities) internal {
        for (uint16 i = 0; i < NUM_AUTHORITIES; i++) {
            authoritySets[authoritySetID][i]  = _authorities[i];
        }
        activeAuthoritySetID = authoritySetID;

        emit AuthoritySetUpdate(activeAuthoritySetID);
    }

    function step(Step memory update) external {
        doStep(update);
    }

    /// @notice Updates the head of the light client with the provided list of headers.
    function doStep(Step memory update) internal {
        // First verify that the authority set is correct.
        if (update.authoritySetIDProof.authoritySetID != activeAuthoritySetID) {
            revert("Authority set ID is not correct");
        }

        // Check to see that the last block's authority set ID is correct.
        bytes32 authSetIDMerkleRoot;
        if (update.headers.length > 1) {
            authSetIDMerkleRoot = update.headers[update.headers.length-2].stateRoot;
        } else {
            authSetIDMerkleRoot = stateRoots[head];
        }

        bytes[] memory keys = new bytes[](1);
        keys[0] = GRANDPA_AUTHORITIES_SETID_KEY;
        bytes memory proof_ret = MerklePatricia.VerifySubstrateProof(authSetIDMerkleRoot,
                                                                     update.authoritySetIDProof.merkleProof,
                                                                     keys)[0];

        if (ScaleCodec.decodeUint64(proof_ret) != update.authoritySetIDProof.authoritySetID) {
            revert("Finalized block authority set proof is not correct");
        }

        // TODO:  Need to implement
        // zkLightClientStep(update.proof, head, headerRoots[head], authoritySets[activeAuthoritySetID]);

        // Note that the snark proof above verifies that the first header is correctly linked to the current head.
        Header memory header;
        for (uint16 i = 0; i < update.headers.length; i ++) {
            header = update.headers[i];
            headerHashes[header.blockNumber] = header.headerHash;
            stateRoots[header.blockNumber] = header.stateRoot;
            dataRoots[header.blockNumber] = header.dataRoot;
        }

        Header memory lastHeader = update.headers[update.headers.length - 1];
        head = lastHeader.blockNumber;

        emit HeadUpdate(lastHeader.blockNumber, lastHeader.headerHash);
    }

    /// @notice Rotates the authority set and will optionally execute a step.
    function rotate(Rotate memory update) external {
        // First call step
        if (update.step.headers.length > 0) {
            doStep(update.step);
        }

        // Verify the new authority set id
        bytes[] memory authSetKeys = new bytes[](1);
        authSetKeys[0] = GRANDPA_AUTHORITIES_SETID_KEY;
        bytes memory authSetProofRet = MerklePatricia.VerifySubstrateProof(stateRoots[head],
                                                                           update.newAuthoritySetIDProof.merkleProof,
                                                                           authSetKeys)[0];

        if (ScaleCodec.decodeUint64(authSetProofRet) != update.newAuthoritySetIDProof.authoritySetID) {
            revert("Incorrect authority set ID committed to the state root");
        }

        // Verify the encoded event list
        bytes[] memory systemEventsKeys = new bytes[](1);
        systemEventsKeys[0] = SYSTEM_EVENTS_KEY;
        bytes memory systemEventsProofRet = MerklePatricia.VerifySubstrateProof(stateRoots[head],
                                                                                update.eventListProof.merkleProof,
                                                                                systemEventsKeys)[0];

        // See here for bytes comparison:  https://ethereum.stackexchange.com/a/99342
        if (systemEventsProofRet.length != update.eventListProof.encodedEventList.length ||
            keccak256(systemEventsProofRet) != keccak256(update.eventListProof.encodedEventList)) {
            revert("Incorrect event list committed to the state root");
        }

        bytes32[NUM_AUTHORITIES] memory newAuthorities = decodeAuthoritySet(update.eventListProof.encodedEventList);
        setAuthorities(update.newAuthoritySetIDProof.authoritySetID, newAuthorities);
    }
}