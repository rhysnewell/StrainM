use itertools::Itertools;
use ordered_float::OrderedFloat;
use std::cmp::min;

use crate::utils::simple_interval::{Locatable, SimpleInterval};
use crate::activity_profile::activity_profile_state::{ActivityProfileState, ActivityProfileDataType};
use crate::assembly::assembly_region::AssemblyRegion;


const PROBABILITY_TOLERANCE_FOR_DENSITY_CHECK: f32 = 0.05;

/**
 * Class holding information about per-base activity scores for
 * assembly region traversal
 *
 * @author Rhys Newell <rhys.newell@hdr.qut.edu.au>
 */
#[derive(Debug, Clone)]
pub struct ActivityProfile {
    pub state_list: Vec<ActivityProfileState>,
    max_prob_propagation_distance: usize,
    active_prob_threshold: f32,
    pub region_start_loc: Option<SimpleInterval>,
    pub region_stop_loc: Option<SimpleInterval>,
    contig_len: usize,
    tid: usize,
    ref_idx: usize,
}

pub trait Profile {
    fn get_max_prob_propagation_distance(&self) -> usize;

    fn size(&self) -> usize;

    fn get_contig(&self) -> usize;

    fn is_empty(&self) -> bool;

    fn get_span(&self) -> Option<SimpleInterval>;

    fn get_end(&self) -> Option<usize>;

    fn get_state_list(&self) -> &Vec<ActivityProfileState>;

    fn get_loc_for_offset(
        &self,
        relative_loc: &SimpleInterval,
        offset: i64,
    ) -> Option<SimpleInterval>;

    fn get_current_contig_length(&self) -> usize;

    fn add(&mut self, state: ActivityProfileState);

    fn process_state(&self, just_added_state: &ActivityProfileState) -> Vec<ActivityProfileState>;

    fn incorporate_single_state(&mut self, state_to_add: ActivityProfileState);

    fn pop_ready_assembly_regions(
        self,
        assembly_region_extension: usize,
        min_region_size: usize,
        max_region_size: usize,
        force_conversion: bool,
    ) -> Vec<AssemblyRegion>;

    fn pop_next_ready_assembly_region(
        &mut self,
        assembly_region_extension: usize,
        min_region_size: usize,
        max_region_size: usize,
        force_conversion: bool,
    ) -> Option<AssemblyRegion>;

    fn find_end_of_region(
        &mut self,
        is_active_region: bool,
        min_region_size: usize,
        max_region_size: usize,
        force_conversion: bool,
    ) -> Option<usize>;

    fn find_best_cut_site(&self, end_of_active_region: usize, min_region_size: usize) -> usize;

    fn find_first_activity_boundary(&self, is_active_region: bool, max_region_size: usize)
        -> usize;

    fn get_prob(&self, index: usize) -> f32;

    fn is_minimum(&self, index: usize) -> bool;

    fn get_probabilities_as_array(&self) -> Vec<f32>;
}

impl ActivityProfile {
    /**
     * Create a empty ActivityProfile, restricting output to profiles overlapping intervals, if not null
     * @param maxProbPropagationDistance region probability propagation distance beyond its maximum size
     * @param activeProbThreshold threshold for the probability of a profile state being active
     */
    pub fn new(
        max_prob_propagation_distance: usize,
        active_prob_threshold: f32,
        ref_idx: usize,
        tid: usize,
        contig_len: usize,
    ) -> ActivityProfile {
        ActivityProfile {
            state_list: Vec::new(),
            max_prob_propagation_distance,
            active_prob_threshold,
            region_start_loc: Some(SimpleInterval::new(0, 0, 0)),
            region_stop_loc: Some(SimpleInterval::new(0, 0, 0)),
            contig_len,
            tid,
            ref_idx,
        }
    }
}

impl Profile for ActivityProfile {
    /**
     * How far away can probability mass be moved around in this profile?
     *
     * This distance puts an upper limit on how far, in bp, we will ever propagate probability mass around
     * when adding a new ActivityProfileState.  For example, if the value of this function is
     * 10, and you are looking at a state at bp 5, and we know that no states beyond 5 + 10 will have
     * their probability propagated back to that state.
     *
     * @return a positive integer distance in bp
     */
    fn get_max_prob_propagation_distance(&self) -> usize {
        self.max_prob_propagation_distance
    }

    /**
     * How many profile results are in this profile?
     * @return the number of profile results
     */
    fn size(&self) -> usize {
        self.state_list.len()
    }

    /**
     * Is this profile empty? (ie., does it contain no ActivityProfileStates?)
     * @return true if the profile is empty (ie., contains no ActivityProfileStates)
     */
    fn is_empty(&self) -> bool {
        self.state_list.is_empty()
    }

    /**
     * Get the span of this activity profile, which is from the start of the first state to the stop of the last
     * @return a potentially null SimpleInterval.  Will be null if this profile is empty
     */
    fn get_span(&self) -> Option<SimpleInterval> {
        if self.is_empty() {
            None
        } else {
            if let Some(ref start) = &self.region_start_loc {
                if let Some(ref stop) = &self.region_stop_loc {
                    return Some(start.span_with(stop));
                }
                return None;
            }
            return None;
        }
    }

    fn get_contig(&self) -> usize {
        self.region_start_loc.as_ref().unwrap().get_contig()
    }

    fn get_end(&self) -> Option<usize> {
        if let Some(ref loc) = &self.region_stop_loc {
            return Some(loc.get_end());
        }
        None
    }

    /**
     * Get the list of activity profile results in this object
     * @return a non-null, ordered list of activity profile results
     */
    fn get_state_list(&self) -> &Vec<ActivityProfileState> {
        &self.state_list
    }

    /**
     * Get the probabilities of the states as a single linear array of doubles
     * @return a non-null array
     */
    fn get_loc_for_offset(
        &self,
        relative_loc: &SimpleInterval,
        offset: i64,
    ) -> Option<SimpleInterval> {
        let start = relative_loc.get_start() as i64 + offset;
        if start < 0 || start > self.contig_len as i64 {
            return None;
        } else {
            return Some(SimpleInterval::new(
                self.region_start_loc.as_ref().unwrap().get_contig(),
                start as usize,
                start as usize,
            ));
        }
    }

    /**
     * Get the length of the current contig
     * @return the length in bp
     */
    fn get_current_contig_length(&self) -> usize {
        self.contig_len
    }

    // --------------------------------------------------------------------------------
    //
    // routines to add states to a profile
    //
    // --------------------------------------------------------------------------------

    /**
     * Add the next ActivityProfileState to this profile.
     *
     * Must be contiguous with the previously added result, or an IllegalArgumentException will be thrown
     *
     * @param state a well-formed ActivityProfileState result to incorporate into this profile
     */
    fn add(&mut self, state: ActivityProfileState) {
        let loc = state.get_loc();

        if self.is_empty() {
            self.region_start_loc = Some(loc.clone());
            self.region_stop_loc = Some(loc.clone());
        } else {
            if self.region_stop_loc.as_ref().unwrap().get_start() != loc.get_start() - 1 {
                panic!(
                    "Bad add call to ActivityProfile: loc {:?} not immediately after last loc {:?}",
                    loc, self.region_stop_loc
                )
            }
            self.region_stop_loc = Some(loc.clone());
        }
        let processed_states = self.process_state(&state);

        for processed_state in processed_states.into_iter() {
            self.incorporate_single_state(processed_state)
        }
    }

    /**
     * Incorporate a single activity profile state into the current list of states
     *
     * If state's position occurs immediately after the last position in this profile, then
     * the state is appended to the state list.  If it's within the existing states list,
     * the prob of stateToAdd is added to its corresponding state in the list.  If the
     * position would be before the start of this profile, stateToAdd is simply ignored.
     *
     * @param stateToAdd the state we want to add to the states list
     */
    fn incorporate_single_state(&mut self, state_to_add: ActivityProfileState) {
        let position = state_to_add.get_offset(self.region_start_loc.as_ref().unwrap());
        if position > self.size() as i64 {
            panic!(
                "Must add state contiguous to existing states: adding {:?} position {} size {}",
                state_to_add,
                position,
                self.size()
            )
        }

        if position >= 0 {
            if position < self.size() as i64 {
                let current_prob = self.state_list[position as usize].is_active_prob();
                self.state_list[position as usize]
                    .set_is_active_prob(current_prob + state_to_add.is_active_prob());
            } else {
                if position != self.size() as i64 {
                    panic!(
                        "Position is meant to == size, but it did not {:?}",
                        state_to_add
                    )
                }
                self.state_list.push(state_to_add)
            }
        }
    }

    /**
     * Process justAddedState, returning a collection of derived states that actually be added to the stateList
     *
     * The purpose of this function is to transform justAddedStates, if needed, into a series of atomic states
     * that we actually want to track.  For example, if state is for soft clips, we transform that single
     * state into a list of states that surround the state up to the distance of the soft clip.
     *
     * Can be overridden by subclasses to transform states in any way
     *
     * There's no particular contract for the output states, except that they can never refer to states
     * beyond the current end of the stateList unless the explicitly include preceding states before
     * the reference.  So for example if the current state list is [1, 2, 3] this function could return
     * [1,2,3,4,5] but not [1,2,3,5].
     *
     * @param justAddedState the state our client provided to use to add to the list
     * @return a list of derived states that should actually be added to this profile's state list
     */
    fn process_state(&self, just_added_state: &ActivityProfileState) -> Vec<ActivityProfileState> {
        // debug!("Just added {:?}", just_added_state);
        match just_added_state.get_result_state() {
            ActivityProfileDataType::HighQualitySoftClips(num_hq_clips) => {
                // special code to deal with the problem that high quality soft clipped bases aren't added to pileups
                let mut states = Vec::new();
                // add no more than the max prob propagation distance num HQ clips
                let num_hq_clips = std::cmp::min(
                    OrderedFloat(*num_hq_clips),
                    OrderedFloat(self.max_prob_propagation_distance as f32),
                )
                .into_inner() as i64;
                // debug!("Num HQ clips {:?}", num_hq_clips);
                for i in (-num_hq_clips..=num_hq_clips).into_iter() {
                    let loc = self.get_loc_for_offset(just_added_state.get_loc(), i);
                    match loc {
                        Some(loc) => states.push(ActivityProfileState::new(
                            loc,
                            just_added_state.is_active_prob(),
                            ActivityProfileDataType::None,
                        )),
                        _ => {
                            // Do nothing
                        }
                    }
                }
                // debug!("Soft clips added {}", states.len());
                return states;
            }
            ActivityProfileDataType::None => {
                vec![just_added_state.clone()]
            }
        }
    }

    // --------------------------------------------------------------------------------
    //
    // routines to get active regions from the profile
    //
    // --------------------------------------------------------------------------------

    /**
     * Get the next completed assembly regions from this profile, and remove all states supporting them from this profile
     *
     * Takes the current profile and finds all of the active / inactive from the start of the profile that are
     * ready.  By ready we mean unable to have their probability modified any longer by future additions to the
     * profile.  The regions that are popped off the profile take their states with them, so the start of this
     * profile will always be after the end of the last region returned here.
     *
     * The regions are returned sorted by genomic position.
     *
     * This function may not return anything in the list, if no regions are ready
     *
     * No returned region will be larger than maxRegionSize.
     *
     * @param assemblyRegionExtension the extension value to provide to the constructed regions
     * @param minRegionSize the minimum region size, in the case where we have to cut up regions that are too large
     * @param maxRegionSize the maximize size of the returned region
     * @param forceConversion if true, we'll return a region whose end isn't sufficiently far from the end of the
     *                        stateList.  Used to close out the active region when we've hit some kind of end (such
     *                        as the end of the contig)
     * @return a non-null list of active regions
     */
    fn pop_ready_assembly_regions(
        mut self,
        assembly_region_extension: usize,
        min_region_size: usize,
        max_region_size: usize,
        _force_conversion: bool,
    ) -> Vec<AssemblyRegion> {
        assert!(min_region_size > 0, "min_region_size must be >= 1");
        assert!(max_region_size > 0, "max_region_size must be >= 1");

        let mut regions = Vec::new();
        let mut region_start = None;
        loop {
            let force_conversion = if let Some(start) = region_start {
                if let Some(end) = self.get_end() {
                    start != end + 1
                } else {
                    false
                }
            } else {
                false
            };
            // let force_conversion = false;

            // debug!("Force conversion {}", force_conversion);
            let next_region = self.pop_next_ready_assembly_region(
                assembly_region_extension,
                min_region_size,
                max_region_size,
                force_conversion,
            );

            match next_region {
                Some(region) => {
                    region_start = Some(region.active_span.start);
                    // debug!("Next region {:?}", &region.active_span);
                    regions.push(region);
                }
                None => return regions,
            }
        }
    }

    /**
     * Helper function for popReadyActiveRegions that pops the first ready region off the front of this profile
     *
     * If a region is returned, modifies the state of this profile so that states used to make the region are
     * no longer part of the profile.  Associated information (like the region start position) of this profile
     * are also updated.
     *
     * @param assemblyRegionExtension the extension value to provide to the constructed regions
     * @param minRegionSize the minimum region size, in the case where we have to cut up regions that are too large
     * @param maxRegionSize the maximize size of the returned region
     * @param forceConversion if true, we'll return a region whose end isn't sufficiently far from the end of the
     *                        stateList.  Used to close out the active region when we've hit some kind of end (such
     *                        as the end of the contig)
     * @return a fully formed assembly region, or null if none can be made
     */
    fn pop_next_ready_assembly_region(
        &mut self,
        assembly_region_extension: usize,
        min_region_size: usize,
        max_region_size: usize,
        force_conversion: bool,
    ) -> Option<AssemblyRegion> {
        if self.state_list.is_empty() {
            return None;
        }
        // If we are flushing the activity profile we need to trim off the excess
        // states so that we don't create regions outside of our current processing interval
        // if force_conversion {
        //     let span = self.get_span();
        //     match span {
        //         Some(span) => {
        //             // self.state_list = &self.state_list[span.size()..];
        //             // self.state_list.retain(|state| !states_to_trim_away.contains(state));
        //             debug!("Span size {} states {}", span.size(), self.state_list.len());
        //             if span.size() < self.state_list.len() {
        //                 // debug!(
        //                 //     "Drained {}",
        //                 //     self.state_list
        //                 //         .drain(span.size()..self.state_list.len())
        //                 //         .count()
        //                 // );
        //                 // self.state_list = self.state_list[0..span.size()].to_vec();
        //             }
        //         }
        //         None => {
        //             // Do nothing I guess?
        //         }
        //     }
        // }

        // debug!("Active prob 0 {} Threshold {}", &self.state_list[0].is_active_prob(), &self.active_prob_threshold);
        let is_active_region = &self.state_list[0].is_active_prob() > &self.active_prob_threshold;
        // debug!(
        //     "First {:?} active? {}",
        //     &self.state_list[0], is_active_region
        // );
        let offset_of_next_region_end = self.find_end_of_region(
            is_active_region,
            min_region_size,
            max_region_size,
            force_conversion,
        );

        // debug!("Offset {:?}", &offset_of_next_region_end);
        match offset_of_next_region_end {
            Some(offset_of_next_region_end) => {
                // we need to create the active region, and clip out the states we're extracting from this profile
                let sub = self
                    .state_list
                    .drain(0..offset_of_next_region_end + 1)
                    .collect_vec();

                // update the start and stop locations as necessary
                if self.state_list.is_empty() {
                    self.region_start_loc = None;
                    self.region_stop_loc = None;
                } else {
                    self.region_start_loc = Some(self.state_list[0].get_loc().clone());
                }

                let first = &sub[0]; // first is the first active state BEFORe draining
                let region_loc = SimpleInterval::new(
                    first.get_loc().get_contig(),
                    first.get_loc().get_start(),
                    min(
                        first.get_loc().get_start() + offset_of_next_region_end,
                        self.contig_len - 1,
                    ),
                );

                // we can get a glimpse of the activity density here. i.e. the number of active states
                // within this span
                let activity_density = sub.iter().filter(|state| state.is_active_prob() > PROBABILITY_TOLERANCE_FOR_DENSITY_CHECK).count();
                // divide this density count by the length
                let activity_density = activity_density as f32 / region_loc.size() as f32;

                // debug!("regionLoc {:?}: activity density {}", &region_loc, activity_density);
                return Some(AssemblyRegion::new(
                    region_loc,
                    is_active_region,
                    assembly_region_extension,
                    self.contig_len,
                    self.tid,
                    self.ref_idx,
                    activity_density,
                ));
            }
            None => None,
        }
    }

    /**
     * Find the end of the current region, returning the index into the element isActive element, or -1 if the region isn't done
     *
     * The current region is defined from the start of the stateList, looking for elements that have the same isActiveRegion
     * flag (i.e., if isActiveRegion is true we are looking for states with isActiveProb > threshold, or alternatively
     * for states < threshold).  The maximize size of the returned region is maxRegionSize.  If forceConversion is
     * true, then we'll return the region end even if this isn't safely beyond the max prob propagation distance.
     *
     * Note that if isActiveRegion is true, and we can construct an assembly region > maxRegionSize in bp, we
     * find the further local minimum within that max region, and cut the region there, under the constraint
     * that the resulting region must be at least minRegionSize in bp.
     *
     * @param isActiveRegion is the region we're looking for an active region or inactive region?
     * @param minRegionSize the minimum region size, in the case where we have to cut up regions that are too large
     * @param maxRegionSize the maximize size of the returned region
     * @param forceConversion if true, we'll return a region whose end isn't sufficiently far from the end of the
     *                        stateList.  Used to close out the assembly region when we've hit some kind of end (such
     *                        as the end of the contig)
     * @return the index into stateList of the last element of this region, or -1 if it cannot be found
     */
    fn find_end_of_region(
        &mut self,
        is_active_region: bool,
        min_region_size: usize,
        max_region_size: usize,
        force_conversion: bool,
    ) -> Option<usize> {
        if !force_conversion
            && self.state_list.len() < max_region_size + self.max_prob_propagation_distance
        {
            // we really haven't finalized at the probability mass that might affect our decision, so keep
            // waiting until we do before we try to make any decisions
            return None;
        }

        let mut end_of_active_region =
            self.find_first_activity_boundary(is_active_region, max_region_size);
        // debug!("Find end 1 {}", end_of_active_region);
        if is_active_region && (end_of_active_region == max_region_size) {
            end_of_active_region = self.find_best_cut_site(end_of_active_region, min_region_size);
            // debug!("Find end 2 {}", end_of_active_region);
        }

        return end_of_active_region.checked_sub(1);
    }

    /**
     * Find the the local minimum within 0 - endOfActiveRegion where we should divide region
     *
     * This algorithm finds the global minimum probability state within the region [minRegionSize, endOfActiveRegion)
     * (exclusive of endOfActiveRegion), and returns the state index of that state.
     * that it
     *
     * @param endOfActiveRegion the last state of the current active region (exclusive)
     * @param minRegionSize the minimum of the left-most region, after cutting
     * @return the index of state after the cut site (just like endOfActiveRegion)
     */
    fn find_best_cut_site(&self, end_of_active_region: usize, min_region_size: usize) -> usize {
        assert!(
            end_of_active_region >= min_region_size,
            "end_of_active_region must be >= min_region_size"
        );

        let mut min_i = end_of_active_region - 1;
        let mut min_p = std::f32::MAX;

        let mut i = min_i;
        while i >= min_region_size {
            let cur = self.get_prob(i);
            if cur < min_p && self.is_minimum(i) {
                min_p = cur;
                min_i = i;
            }
            i -= 1;
        }
        // for i in ((min_region_size - 1)..=min_i).into_iter().rev() {
        //
        // }

        return min_i + 1;
    }

    /**
     * Find the first index into the state list where the state is considered ! isActiveRegion
     *
     * Note that each state has a probability of being active, and this function thresholds that
     * value on activeProbThreshold, coloring each state as active or inactive.  Finds the
     * largest contiguous stretch of states starting at the first state (index 0) with the same isActive
     * state as isActiveRegion.  If the entire state list has the same isActive value, then returns
     * maxRegionSize
     *
     * @param isActiveRegion are we looking for a stretch of active states, or inactive ones?
     * @param maxRegionSize don't look for a boundary that would yield a region of size > maxRegionSize
     * @return the index of the first state in the state list with isActive value != isActiveRegion, or maxRegionSize
     *         if no such element exists
     */
    fn find_first_activity_boundary(
        &self,
        is_active_region: bool,
        max_region_size: usize,
    ) -> usize {
        let n_states = self.state_list.len();

        let mut end_of_active_region = 0;

        while end_of_active_region < n_states && end_of_active_region < max_region_size {
            if (self.get_prob(end_of_active_region) > self.active_prob_threshold)
                != is_active_region
            {
                // debug!("Active {}", self.get_prob(end_of_active_region));
                break;
            }
            end_of_active_region += 1;
        }
        return end_of_active_region;
    }

    /**
     * Helper function to get the probability of the state at offset index
     * @param index a valid offset into the state list
     * @return the isActiveProb of the state at index
     */
    fn get_prob(&self, index: usize) -> f32 {
        return self.state_list[index].is_active_prob();
    }

    /**
     * Is the probability at index in a local minimum?
     *
     * Checks that the probability at index is <= both the probabilities to either side.
     * Returns false if index is at the end or the start of the state list.
     *
     * @param index the index of the state we want to test
     * @return true if prob at state is a minimum, false otherwise
     */
    fn is_minimum(&self, index: usize) -> bool {
        if index == self.state_list.len() - 1 || index < 1 {
            // we cannot be at a minimum if the current position is the last in the state list
            return false;
        } else {
            let index_p = self.get_prob(index);
            return index_p <= self.get_prob(index + 1) && index_p < self.get_prob(index - 1);
        }
    }

    fn get_probabilities_as_array(&self) -> Vec<f32> {
        let probs = self
            .get_state_list()
            .into_iter()
            .map(|state| state.is_active_prob())
            .collect::<Vec<f32>>();
        return probs;
    }
}

/**
* Implement the extend method for ActivityProfile when
* given a parallel iterator of ActivityProfileState
*/
impl Extend<ActivityProfileState> for ActivityProfile {
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = ActivityProfileState>,
    {
        let iter = iter.into_iter();
        iter.for_each(|state| self.add(state));
    }
}
