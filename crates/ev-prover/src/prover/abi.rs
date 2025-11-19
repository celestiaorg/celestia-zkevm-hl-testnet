use alloy::sol;
sol! {
    #[sol(rpc)]
    contract MailboxContract {
        function nonce() public view returns (uint32);
    }
}
