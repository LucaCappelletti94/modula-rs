-- The Newman-modularity efficiency (modularity_term), the AMI divergence
-- (divergence_term), and the depth-averaged headline are replaced by a single
-- hub-tolerant cohesion-lift term. The community detector and depth sweep that
-- produced the first two are gone.
ALTER TABLE analyses DROP COLUMN modularity_term;
ALTER TABLE analyses DROP COLUMN divergence_term;
ALTER TABLE analyses DROP COLUMN headline_depth_averaged;
ALTER TABLE analyses ADD COLUMN cohesion_term DOUBLE;
