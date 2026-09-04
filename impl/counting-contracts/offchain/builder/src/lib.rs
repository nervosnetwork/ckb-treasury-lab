use counting_common::{CountingCellData, Hash, OutPoint};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoteCell {
    pub out_point: OutPoint,
    pub lock_hash: Hash,
    pub direction: u8,
    pub amount: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountingBatch {
    pub data: CountingCellData,
    pub vote_cells: Vec<VoteCell>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildError {
    InvalidDirection,
    InvalidVote,
    DuplicateVoterLock,
    BucketTooLarge,
    AmountOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CountingBuilder {
    max_votes_per_cell: u32,
}

impl CountingBuilder {
    pub fn new(max_votes_per_cell: u32) -> Result<Self, BuildError> {
        if max_votes_per_cell == 0 {
            return Err(BuildError::InvalidVote);
        }
        Ok(Self { max_votes_per_cell })
    }

    pub fn build(
        &self,
        direction: u8,
        mut votes: Vec<VoteCell>,
    ) -> Result<Vec<CountingBatch>, BuildError> {
        if direction > 1 {
            return Err(BuildError::InvalidDirection);
        }
        if votes
            .iter()
            .any(|vote| vote.direction != direction || vote.amount == 0)
        {
            return Err(BuildError::InvalidVote);
        }
        votes.sort_unstable_by_key(|vote| vote.lock_hash);
        if votes
            .windows(2)
            .any(|pair| pair[0].lock_hash == pair[1].lock_hash)
        {
            return Err(BuildError::DuplicateVoterLock);
        }

        let mut batches = Vec::new();
        let mut batch_votes = Vec::new();
        let mut batch_start = 0u8;
        let mut batch_end = 0u8;
        let mut offset = 0usize;
        while offset < votes.len() {
            let bucket = votes[offset].lock_hash[0];
            let bucket_end = votes[offset..]
                .iter()
                .position(|vote| vote.lock_hash[0] != bucket)
                .map_or(votes.len(), |relative| offset + relative);
            let bucket_len = bucket_end - offset;
            if bucket_len > self.max_votes_per_cell as usize {
                return Err(BuildError::BucketTooLarge);
            }
            if !batch_votes.is_empty()
                && batch_votes.len() + bucket_len > self.max_votes_per_cell as usize
            {
                batches.push(build_batch(
                    direction,
                    batch_start,
                    batch_end,
                    core::mem::take(&mut batch_votes),
                )?);
            }
            if batch_votes.is_empty() {
                batch_start = bucket;
            }
            batch_end = bucket;
            batch_votes.extend_from_slice(&votes[offset..bucket_end]);
            offset = bucket_end;
        }
        if !batch_votes.is_empty() {
            batches.push(build_batch(direction, batch_start, batch_end, batch_votes)?);
        }
        Ok(batches)
    }
}

fn build_batch(
    direction: u8,
    range_start: u8,
    range_end: u8,
    vote_cells: Vec<VoteCell>,
) -> Result<CountingBatch, BuildError> {
    let amount = vote_cells.iter().try_fold(0u128, |sum, vote| {
        sum.checked_add(vote.amount as u128)
            .ok_or(BuildError::AmountOverflow)
    })?;
    let vote_count = vote_cells
        .len()
        .try_into()
        .map_err(|_| BuildError::AmountOverflow)?;
    Ok(CountingBatch {
        data: CountingCellData {
            direction,
            range_start,
            range_end,
            amount,
            vote_count,
        },
        vote_cells,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vote(first_byte: u8, tail: u8, direction: u8, amount: u64) -> VoteCell {
        let mut lock_hash = [tail; 32];
        lock_hash[0] = first_byte;
        VoteCell {
            out_point: OutPoint {
                tx_hash: [tail; 32],
                index: 0,
            },
            lock_hash,
            direction,
            amount,
        }
    }

    #[test]
    fn builds_sorted_non_overlapping_ranges_without_splitting_a_bucket() {
        let builder = CountingBuilder::new(3).unwrap();
        let batches = builder
            .build(
                1,
                vec![
                    vote(0x20, 3, 1, 30),
                    vote(0x10, 1, 1, 10),
                    vote(0x20, 2, 1, 20),
                    vote(0x30, 4, 1, 40),
                ],
            )
            .unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(
            (batches[0].data.range_start, batches[0].data.range_end),
            (0x10, 0x20)
        );
        assert_eq!(batches[0].data.amount, 60);
        assert_eq!(batches[0].data.vote_count, 3);
        assert_eq!(
            (batches[1].data.range_start, batches[1].data.range_end),
            (0x30, 0x30)
        );
        assert_eq!(batches[1].data.amount, 40);
    }

    #[test]
    fn rejects_duplicate_locks_and_oversized_first_byte_buckets() {
        let builder = CountingBuilder::new(1).unwrap();
        let duplicate = vote(1, 2, 1, 10);
        assert_eq!(
            builder.build(1, vec![duplicate.clone(), duplicate]),
            Err(BuildError::DuplicateVoterLock)
        );
        assert_eq!(
            builder.build(1, vec![vote(1, 2, 1, 10), vote(1, 3, 1, 20)]),
            Err(BuildError::BucketTooLarge)
        );
    }
}
